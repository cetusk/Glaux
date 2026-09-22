//! RT セーフなレンダラ。オーディオスレッド(cpal コールバック)から呼ばれる。
//!
//! 絶対条件(CLAUDE.md): このモジュールの `process` 内では
//! **アロケーション・ロック・ブロッキング I/O をしない**。
//! - 制御は [`Shared`] のアトミックと `ArcSwap` 経由(どちらもロックフリー)
//! - ボイス配列は起動時に確保した固定容量(`MAX_VOICES`)を使い回す
//! - `PlaybackData` の解放はオーディオスレッドでは起きない
//!   (ハンドル側が旧データを graveyard に保持してから捨てる)
//!
//! 音源は glaux-dsp の内蔵楽器(subtractive / drum)。トラックの device 設定から
//! 焼き込まれたパラメータ([`crate::data::TrackMix::instrument`])で発音する。

use crate::data::{db_to_amp, pan_gains, AutoPoint, PlaybackData, MAX_EFFECT_SLOTS, MAX_TRACKS};
use arc_swap::ArcSwap;
use glaux_core::Curve;
use glaux_dsp::{EffectParams, EffectState, VoiceState};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

pub const MAX_VOICES: usize = 64;
/// 同時プレビュー(試聴)ボイス数
pub const MAX_PREVIEW_VOICES: usize = 8;
/// シーク要求なしを表す番兵値
pub const NO_SEEK: u64 = u64::MAX;
/// リリースが終わらないボイスの強制解放(秒)。スタック防止の保険
const VOICE_HARD_LIMIT_SECS: f32 = 8.0;
/// 最後のノートが終わってから自動停止するまでの余韻(秒)
const TAIL_SECS: f64 = 2.0;

/// ハンドル(UI 側)とレンダラ(オーディオ側)が共有する制御データ。
pub struct Shared {
    pub playing: AtomicBool,
    /// 現在の再生位置(サンプル)。レンダラが書き、UI が読む
    pub pos: AtomicU64,
    /// シーク要求(サンプル)。`NO_SEEK` なら要求なし。UI が書き、レンダラが消費する
    pub seek: AtomicU64,
    /// ノート試聴要求(パック形式)。UI が書き、レンダラがカウンタ変化で検出する。
    /// bits: [63:48]=カウンタ [47:32]=トラック index [31:16]=長さ(ms) [15:8]=pitch [7:0]=vel
    pub preview: AtomicU64,
    /// ループ区間(サンプル)。`loop_end <= loop_start` ならループなし。
    /// 2 つのアトミックに分かれているため一瞬だけ不整合になり得るが、
    /// 影響は 1 ブロックのジャンプ位置に限られる(実害なし)
    pub loop_start: AtomicU64,
    pub loop_end: AtomicU64,
    pub data: ArcSwap<PlaybackData>,
}

impl Shared {
    pub fn new(data: PlaybackData) -> Self {
        Shared {
            playing: AtomicBool::new(false),
            pos: AtomicU64::new(0),
            seek: AtomicU64::new(NO_SEEK),
            preview: AtomicU64::new(0),
            loop_start: AtomicU64::new(0),
            loop_end: AtomicU64::new(0),
            data: ArcSwap::from_pointee(data),
        }
    }
}

#[derive(Clone, Copy)]
struct Voice {
    end: u64,
    track: u32,
    released: bool,
    state: VoiceState,
}

/// UI からの試聴用ボイス。停止中でも鳴り、トラックエフェクトは通さない。
#[derive(Clone, Copy)]
struct PreviewVoice {
    /// note_off までの残りサンプル数
    remaining: u32,
    released: bool,
    gain_l: f32,
    gain_r: f32,
    instrument: glaux_dsp::InstrumentParams,
    state: VoiceState,
}

pub struct Renderer {
    shared: Arc<Shared>,
    voices: Vec<Voice>,
    preview_voices: Vec<PreviewVoice>,
    /// 最後に消費した試聴要求のカウンタ
    last_preview: u64,
    /// エフェクト状態プール(リバーブのバッファ込みで起動時に確保)
    effect_states: Vec<EffectState>,
    /// トラックごとの無音連続サンプル数(残響が消えたらエフェクト処理を省く)
    track_silence: [u32; MAX_TRACKS],
    /// オートメーション評価カーソル(vol, pan)。単調前進、resync でリセット
    auto_cursors: [(usize, usize); MAX_TRACKS],
    /// `data.events` の次に発音するイベントの添字
    next_event: usize,
    /// 直前に見ていた `PlaybackData` のアドレス(差し替え検出用)
    last_data: usize,
    pos: u64,
}

/// オートメーション点列を区分補間で評価する(core の `AutomationLane::value_at` と同義)。
/// `cursor` は「sample <= pos の最後の点」の添字で、単調に前進させる。
fn eval_auto(points: &[AutoPoint], cursor: &mut usize, pos: u64) -> f32 {
    while *cursor + 1 < points.len() && points[*cursor + 1].sample <= pos {
        *cursor += 1;
    }
    let a = points[*cursor];
    if pos < a.sample {
        return a.value; // 最初の点より前
    }
    let Some(b) = points.get(*cursor + 1) else {
        return a.value; // 最後の点より後
    };
    let span = (b.sample - a.sample) as f32;
    if span <= 0.0 {
        return a.value;
    }
    let t = (pos - a.sample) as f32 / span;
    match a.curve {
        Curve::Hold => a.value,
        Curve::Linear => a.value + (b.value - a.value) * t,
        Curve::Exponential => a.value + (b.value - a.value) * t * t,
    }
}

impl Renderer {
    pub fn new(shared: Arc<Shared>) -> Self {
        Renderer {
            shared,
            voices: Vec::with_capacity(MAX_VOICES),
            preview_voices: Vec::with_capacity(MAX_PREVIEW_VOICES),
            last_preview: 0,
            effect_states: vec![EffectState::default(); MAX_EFFECT_SLOTS],
            track_silence: [u32::MAX; MAX_TRACKS],
            auto_cursors: [(0, 0); MAX_TRACKS],
            next_event: 0,
            last_data: 0,
            pos: 0,
        }
    }

    /// `out` はインターリーブされた出力バッファ。
    pub fn process(&mut self, out: &mut [f32], channels: usize) {
        out.fill(0.0);
        if channels == 0 {
            return;
        }

        let guard = self.shared.data.load();
        let data: &PlaybackData = &guard;
        let data_addr = std::sync::Arc::as_ptr(&guard) as usize;

        // データ差し替え・シークのどちらでも発音状態を作り直す
        let mut resync = false;
        if data_addr != self.last_data {
            self.last_data = data_addr;
            resync = true;
        }
        let seek = self.shared.seek.swap(NO_SEEK, Ordering::AcqRel);
        if seek != NO_SEEK {
            self.pos = seek;
            resync = true;
        }
        if resync {
            self.voices.clear();
            self.auto_cursors = [(0, 0); MAX_TRACKS];
            self.next_event = data.events.partition_point(|e| e.start < self.pos);
            // スロットの中身が変わっていたらエフェクト状態を作り直す(アロケーションなし)
            for fx in data
                .tracks
                .iter()
                .flat_map(|t| t.effects.iter())
                .chain(data.master_effects.iter())
            {
                self.effect_states[fx.slot as usize].ensure_kind(&fx.params);
            }
        }

        let sr = data.sample_rate as f32;

        // ノート試聴要求(カウンタ変化で 1 回だけ発音)
        let preview_req = self.shared.preview.load(Ordering::Acquire);
        if preview_req != self.last_preview && preview_req != 0 {
            self.last_preview = preview_req;
            let track = ((preview_req >> 32) & 0xFFFF) as usize;
            let dur_ms = ((preview_req >> 16) & 0xFFFF) as u32;
            let pitch = ((preview_req >> 8) & 0xFF) as u8;
            let vel = (preview_req & 0xFF) as u8;
            let (instrument, gain_l, gain_r) = match data.tracks.get(track) {
                Some(mix) => (mix.instrument, mix.gain_l, mix.gain_r),
                None => (glaux_dsp::InstrumentParams::default(), 0.8, 0.8),
            };
            if self.preview_voices.len() < MAX_PREVIEW_VOICES {
                let freq = crate::data::pitch_to_freq(pitch);
                self.preview_voices.push(PreviewVoice {
                    remaining: (dur_ms as f32 / 1000.0 * sr) as u32,
                    released: false,
                    gain_l,
                    gain_r,
                    instrument,
                    state: VoiceState::start(
                        &instrument,
                        freq,
                        pitch,
                        vel as f32 / 127.0,
                        glaux_core::Articulation::Normal,
                        sr,
                    ),
                });
            }
        }

        let playing = self.shared.playing.load(Ordering::Acquire);
        if !playing {
            self.voices.clear();
            if self.preview_voices.is_empty() {
                self.shared.pos.store(self.pos, Ordering::Release);
                return;
            }
        }

        let hard_limit = (VOICE_HARD_LIMIT_SECS * sr) as u64;
        let frames = out.len() / channels;

        // ループ区間(このブロックの間は固定値として扱う)
        let loop_start = self.shared.loop_start.load(Ordering::Acquire);
        let loop_end = self.shared.loop_end.load(Ordering::Acquire);
        let looping = loop_end > loop_start;

        for frame in 0..frames {
            // ループ終端に達したら区間頭へ。発音中の音は note_off でリリースに回し
            // (ぶつ切りのクリックを避ける)、イベント・オートメーションのカーソルを再同期
            if playing && looping && self.pos >= loop_end {
                self.pos = loop_start;
                for v in &mut self.voices {
                    if !v.released {
                        v.state.note_off();
                        v.released = true;
                    }
                }
                self.auto_cursors = [(0, 0); MAX_TRACKS];
                self.next_event = data.events.partition_point(|e| e.start < self.pos);
            }

            // このサンプル位置で始まるノートを発音(容量超過分は捨てる)
            while playing
                && self.next_event < data.events.len()
                && data.events[self.next_event].start <= self.pos
            {
                let e = data.events[self.next_event];
                self.next_event += 1;
                let mix = data.tracks.get(e.track as usize);
                if let Some(mix) = mix.filter(|m| m.audible) {
                    if self.voices.len() < MAX_VOICES {
                        self.voices.push(Voice {
                            end: e.end,
                            track: e.track,
                            released: false,
                            state: VoiceState::start(
                                &mix.instrument,
                                e.freq,
                                e.pitch,
                                e.amp,
                                e.articulation,
                                sr,
                            ),
                        });
                    }
                }
            }

            // トラックごとのモノ合算(エフェクト前)
            let mut track_mono = [0.0f32; MAX_TRACKS];
            let mut direct_l = 0.0f32; // MAX_TRACKS 超のトラックはエフェクトなしで直行
            let mut direct_r = 0.0f32;
            let mut i = 0;
            while i < self.voices.len() {
                let v = &mut self.voices[i];
                // resync 直後以外で track が範囲外になることはない
                let Some(mix) = data.tracks.get(v.track as usize) else {
                    self.voices.swap_remove(i);
                    continue;
                };
                if self.pos >= v.end && !v.released {
                    v.state.note_off();
                    v.released = true;
                }
                if (v.released && v.state.finished(&mix.instrument))
                    || self.pos >= v.end + hard_limit
                {
                    self.voices.swap_remove(i);
                    continue;
                }
                let sample = v.state.next(&mix.instrument);
                match track_mono.get_mut(v.track as usize) {
                    Some(acc) => *acc += sample,
                    None => {
                        direct_l += sample * mix.gain_l;
                        direct_r += sample * mix.gain_r;
                    }
                }
                i += 1;
            }

            // サイドチェインの検出信号: ソーストラックの生ミックス(エフェクト前)。
            // track_mono はこの時点で全トラック分確定しているので、処理順に依存しない
            let sidechain_key = |p: &EffectParams| -> f32 {
                if let EffectParams::Sidechain(sc) = p {
                    track_mono
                        .get(sc.source_track as usize)
                        .copied()
                        .unwrap_or(0.0)
                } else {
                    0.0
                }
            };

            // トラックごとに 楽器合算 → エフェクトチェーン → 音量/パン → マスター
            // (エフェクトの残響はボイスが消えた後も続くので、毎フレーム全トラックを回す)
            let mut l = direct_l;
            let mut r = direct_r;
            // 残響テールが確実に消えるまでの猶予(これを超えて無音ならエフェクトを省く)
            let tail_limit = (4.0 * sr) as u32;
            for (ti, mix) in data.tracks.iter().take(MAX_TRACKS).enumerate() {
                let mono = track_mono[ti];
                if mono == 0.0 {
                    self.track_silence[ti] = self.track_silence[ti].saturating_add(1);
                } else {
                    self.track_silence[ti] = 0;
                }
                // 音量・パン: オートメーションレーンがあればフェーダーより優先
                let (gl, gr) = if mix.vol_db_auto.is_empty() && mix.pan_auto.is_empty() {
                    (mix.gain_l, mix.gain_r)
                } else {
                    let (vol_cur, pan_cur) = &mut self.auto_cursors[ti];
                    let amp = if mix.vol_db_auto.is_empty() {
                        mix.base_amp
                    } else {
                        db_to_amp(eval_auto(&mix.vol_db_auto, vol_cur, self.pos))
                    };
                    let pan = if mix.pan_auto.is_empty() {
                        mix.base_pan
                    } else {
                        eval_auto(&mix.pan_auto, pan_cur, self.pos).clamp(-1.0, 1.0)
                    };
                    let (pl, pr) = pan_gains(pan);
                    (amp * pl, amp * pr)
                };

                if mix.effects.is_empty() {
                    l += mono * gl;
                    r += mono * gr;
                    continue;
                }
                // 長く無音のトラックはチェーンごとスキップ(CPU 節約)
                if self.track_silence[ti] > tail_limit {
                    continue;
                }
                let (mut fl, mut fr) = (mono, mono);
                for fx in &mix.effects {
                    let key = sidechain_key(&fx.params);
                    let state = &mut self.effect_states[fx.slot as usize];
                    (fl, fr) = state.process(&fx.params, fl, fr, key);
                }
                l += fl * gl;
                r += fr * gr;
            }

            // 試聴ボイス(停止中でも鳴る。トラックエフェクトはバイパス)
            let mut i = 0;
            while i < self.preview_voices.len() {
                let v = &mut self.preview_voices[i];
                if v.remaining == 0 && !v.released {
                    v.state.note_off();
                    v.released = true;
                }
                if v.released && v.state.finished(&v.instrument) {
                    self.preview_voices.swap_remove(i);
                    continue;
                }
                v.remaining = v.remaining.saturating_sub(1);
                let sample = v.state.next(&v.instrument);
                l += sample * v.gain_l;
                r += sample * v.gain_r;
                i += 1;
            }

            // マスターバスのエフェクト → マスター音量 → ソフトクリップ
            for fx in &data.master_effects {
                let key = sidechain_key(&fx.params);
                let state = &mut self.effect_states[fx.slot as usize];
                (l, r) = state.process(&fx.params, l, r, key);
            }
            let base = frame * channels;
            out[base] = (l * data.master_amp).tanh();
            if channels >= 2 {
                out[base + 1] = (r * data.master_amp).tanh();
            }
            if playing {
                self.pos += 1;
            }
        }

        // 曲が終わって余韻も消えたら自動停止(ループ中は止めない)
        let tail = (TAIL_SECS * data.sample_rate) as u64;
        if playing && !looping && data.end_sample > 0 && self.pos > data.end_sample + tail {
            self.shared.playing.store(false, Ordering::Release);
        }

        self.shared.pos.store(self.pos, Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::{NoteEvent, TrackMix};
    use glaux_dsp::{InstrumentParams, SubtractiveParams, Waveform};

    /// テスト用: リリースの短いサイン波 subtractive(旧サイン波シンセ相当)
    fn test_instrument() -> InstrumentParams {
        InstrumentParams::Subtractive(SubtractiveParams {
            waveform: Waveform::Sine,
            cutoff: 12_000.0,
            resonance: 0.0,
            attack: 0.001,
            decay: 1.0,
            sustain: 1.0,
            release: 0.01,
            filter_env: 0.0,
            unison: 1,
            detune_cents: 0.0,
            sub: 0.0,
            noise: 0.0,
            gain: 1.0,
        })
    }

    fn data_with_note(start: u64, end: u64, audible: bool) -> PlaybackData {
        PlaybackData {
            events: vec![NoteEvent {
                articulation: Default::default(),
                start,
                end,
                freq: 440.0,
                pitch: 69,
                amp: 1.0,
                track: 0,
            }],
            tracks: vec![TrackMix {
                gain_l: 1.0,
                gain_r: 1.0,
                audible,
                base_amp: 1.0,
                base_pan: 0.0,
                vol_db_auto: vec![],
                pan_auto: vec![],
                instrument: test_instrument(),
                effects: vec![],
            }],
            master_effects: vec![],
            master_amp: 1.0,
            end_sample: end,
            sample_rate: 48_000.0,
        }
    }

    fn rms(buf: &[f32]) -> f32 {
        (buf.iter().map(|s| s * s).sum::<f32>() / buf.len() as f32).sqrt()
    }

    fn render_block(r: &mut Renderer, frames: usize) -> Vec<f32> {
        let mut buf = vec![0.0f32; frames * 2];
        r.process(&mut buf, 2);
        buf
    }

    #[test]
    fn produces_sound_during_note_and_silence_after() {
        let shared = Arc::new(Shared::new(data_with_note(0, 4800, true)));
        shared.playing.store(true, Ordering::Release);
        let mut r = Renderer::new(shared.clone());

        let block = render_block(&mut r, 4800);
        assert!(rms(&block) > 0.1, "ノート区間で音が出るはず");

        // リリース(10ms)より十分先まで進める
        let _ = render_block(&mut r, 4800);
        let block = render_block(&mut r, 4800);
        assert!(rms(&block) < 1e-3, "ノート終了後はほぼ無音のはず");
        assert_eq!(shared.pos.load(Ordering::Acquire), 4800 * 3);
    }

    #[test]
    fn loop_region_wraps_and_retriggers_notes() {
        // ノート 0..4800、ループ区間 0..9600
        let shared = Arc::new(Shared::new(data_with_note(0, 4800, true)));
        shared.playing.store(true, Ordering::Release);
        shared.loop_start.store(0, Ordering::Release);
        shared.loop_end.store(9600, Ordering::Release);
        let mut r = Renderer::new(shared.clone());

        // 1 周目: 前半は鳴り、後半は無音に向かう
        let first = render_block(&mut r, 4800);
        assert!(rms(&first) > 0.1);
        let _ = render_block(&mut r, 4800); // pos = 9600 → 次のブロック頭で巻き戻る

        // 2 周目に入った直後: ノートが再トリガされて再び鳴る
        let wrapped = render_block(&mut r, 4800);
        assert!(rms(&wrapped) > 0.1, "ループ 2 周目でも音が鳴るはず");
        let pos = shared.pos.load(Ordering::Acquire);
        assert!(pos <= 9600, "再生位置がループ区間内に戻るはず: {pos}");
        assert!(shared.playing.load(Ordering::Acquire));
    }

    #[test]
    fn looping_disables_auto_stop() {
        // 曲の終端(4800)+ 余韻 2 秒を大きく超える位置までループ区間を広げても
        // 自動停止しないこと
        let shared = Arc::new(Shared::new(data_with_note(0, 4800, true)));
        shared.playing.store(true, Ordering::Release);
        shared.loop_start.store(0, Ordering::Release);
        shared.loop_end.store(48_000 * 4, Ordering::Release); // 4 秒
        let mut r = Renderer::new(shared.clone());

        // end_sample + tail = 4800 + 96000 = 100800 を超えるまで進める
        for _ in 0..30 {
            let _ = render_block(&mut r, 4800);
        }
        assert!(
            shared.playing.load(Ordering::Acquire),
            "ループ中は自動停止しないはず"
        );

        // ループを解除すると従来どおり自動停止する
        shared.loop_end.store(0, Ordering::Release);
        shared.loop_start.store(0, Ordering::Release);
        let mut stopped = false;
        for _ in 0..40 {
            let _ = render_block(&mut r, 4800);
            if !shared.playing.load(Ordering::Acquire) {
                stopped = true;
                break;
            }
        }
        assert!(stopped, "ループ解除後は曲末で自動停止するはず");
    }

    #[test]
    fn paused_renderer_outputs_silence_and_holds_position() {
        let shared = Arc::new(Shared::new(data_with_note(0, 4800, true)));
        let mut r = Renderer::new(shared.clone());
        let block = render_block(&mut r, 512);
        assert!(rms(&block) < 1e-9);
        assert_eq!(shared.pos.load(Ordering::Acquire), 0);
    }

    #[test]
    fn muted_track_is_silent() {
        let shared = Arc::new(Shared::new(data_with_note(0, 4800, false)));
        shared.playing.store(true, Ordering::Release);
        let mut r = Renderer::new(shared.clone());
        let block = render_block(&mut r, 4800);
        assert!(rms(&block) < 1e-9);
    }

    #[test]
    fn seek_replays_note_and_data_swap_resyncs() {
        let shared = Arc::new(Shared::new(data_with_note(0, 4800, true)));
        shared.playing.store(true, Ordering::Release);
        let mut r = Renderer::new(shared.clone());

        // ノートを過ぎるまで再生
        let _ = render_block(&mut r, 4800 * 3);
        // 先頭にシーク → もう一度鳴る
        shared.seek.store(0, Ordering::Release);
        let block = render_block(&mut r, 4800);
        assert!(rms(&block) > 0.1, "シーク後に再発音するはず");

        // データ差し替え(ノート位置が先) → 現位置より前のイベントは飛ばす
        shared
            .data
            .store(Arc::new(data_with_note(96_000, 100_000, true)));
        let block = render_block(&mut r, 4800);
        assert!(rms(&block) < 1e-6, "差し替え後、未来のノートはまだ鳴らない");
    }

    #[test]
    fn preview_note_sounds_while_paused() {
        let shared = Arc::new(Shared::new(data_with_note(0, 4800, true)));
        let mut r = Renderer::new(shared.clone());
        // 停止中に試聴要求(counter=1, track=0, 100ms, pitch 69, vel 127)
        let packed = (1u64 << 48) | (100u64 << 16) | (69u64 << 8) | 127;
        shared.preview.store(packed, Ordering::Release);
        let block = render_block(&mut r, 4800);
        assert!(rms(&block) > 0.05, "停止中でも試聴は鳴るはず");
        // note_off(100ms)後は減衰して消える
        let _ = render_block(&mut r, 9600);
        let block = render_block(&mut r, 4800);
        assert!(rms(&block) < 1e-3, "試聴は終わるはず");
        // 再生位置は動かない
        assert_eq!(shared.pos.load(Ordering::Acquire), 0);
    }

    #[test]
    fn auto_stops_after_end_plus_tail() {
        let shared = Arc::new(Shared::new(data_with_note(0, 480, true)));
        shared.playing.store(true, Ordering::Release);
        let mut r = Renderer::new(shared.clone());
        // 480 + 96000(tail 2 秒) を超えるまで回す
        for _ in 0..25 {
            let _ = render_block(&mut r, 4800);
        }
        assert!(
            !shared.playing.load(Ordering::Acquire),
            "終端で自動停止するはず"
        );
    }
}
