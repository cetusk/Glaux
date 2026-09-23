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
use crate::midi::{LiveEvent, LiveQueue, LIVE_NO_TRACK};
use arc_swap::ArcSwap;
use glaux_core::Curve;
use glaux_dsp::{EffectParams, EffectState, VoiceState};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;

pub const MAX_VOICES: usize = 64;
/// 同時に再生する音声クリップ数
pub const MAX_AUDIO_VOICES: usize = 16;
/// 同時プレビュー(試聴)ボイス数
pub const MAX_PREVIEW_VOICES: usize = 8;
/// MIDI キーボードのライブ発音ボイス数
pub const MAX_LIVE_VOICES: usize = 32;
/// 1 ブロックで取り出すライブイベントの上限(暴走した入力で処理が伸びないように)
const MAX_LIVE_EVENTS_PER_BLOCK: usize = 256;
/// ライブ演奏の後、停止中でもエフェクトの残響を鳴らし切る時間(秒)
const LIVE_TAIL_SECS: f32 = 4.0;
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
    /// メトロノーム(拍ごとのクリック)を鳴らすか
    pub metronome: AtomicBool,
    /// 録音中(曲末の自動停止を抑止する)
    pub recording: AtomicBool,
    /// メトロノームだけ鳴らす(遅延の較正中。ノート・音声クリップを発音しない)
    pub click_only: AtomicBool,
    pub data: ArcSwap<PlaybackData>,
    /// 負荷の統計(オーディオスレッドが書き、UI が読む)。[`DspStats`] 参照
    pub stats: StatsCounters,
    /// MIDI キーボードのライブ演奏イベント(MIDI 受信スレッドが積み、レンダラが取り出す)
    pub live: LiveQueue,
    /// ライブ演奏の送り先トラック index(`LIVE_NO_TRACK` なら既定音色)
    pub live_track: AtomicU32,
    /// 時刻の基準(`pos_nanos` や MIDI 受信時刻の起点)
    pub epoch: std::time::Instant,
    /// 直前ブロックのフレーム数と、`pos` を書いた時刻(`epoch` からの ns)。
    /// MIDI 録音で「いま聞こえている位置」をブロック内まで推定するのに使う
    pub block_frames: AtomicU32,
    pub pos_nanos: AtomicU64,
}

/// オーディオ処理の負荷統計(アトミック。オーディオスレッドからロックなしで更新)。
#[derive(Default)]
pub struct StatsCounters {
    /// 直近の集計区間の処理時間・予算の合計(ns)と、ブロック負荷の最大(0.1% 単位)
    busy_ns: AtomicU64,
    budget_ns: AtomicU64,
    max_permille: AtomicU64,
    /// 起動からの累計: 処理が予算(ブロック長)を超えた回数
    pub overruns: AtomicU64,
    /// 起動からの累計: 再生中、前回のコールバックからブロック長の 1.8 倍以上
    /// 空いて呼ばれた回数(OS / 他プロセスに CPU を奪われた)
    pub late: AtomicU64,
    /// 起動からの累計: 再生中の再生データ差し替え(発音中の音が切り直される)
    pub swaps: AtomicU64,
}

/// UI に渡す負荷の要約。
#[derive(Clone, Copy, Debug, Default, serde::Serialize)]
pub struct DspStats {
    /// 直近区間の平均負荷(%、処理時間 / ブロック長)
    pub avg_pct: f32,
    /// 直近区間で最も重かったブロックの負荷(%)
    pub max_pct: f32,
    pub overruns: u64,
    pub late: u64,
    pub swaps: u64,
}

impl StatsCounters {
    /// 直近区間の平均・最大を読み出してリセットする(累計カウンタはそのまま)。
    pub fn take(&self) -> DspStats {
        let busy = self.busy_ns.swap(0, Ordering::AcqRel);
        let budget = self.budget_ns.swap(0, Ordering::AcqRel);
        let max = self.max_permille.swap(0, Ordering::AcqRel);
        DspStats {
            avg_pct: if budget > 0 {
                busy as f32 / budget as f32 * 100.0
            } else {
                0.0
            },
            max_pct: max as f32 / 10.0,
            overruns: self.overruns.load(Ordering::Acquire),
            late: self.late.load(Ordering::Acquire),
            swaps: self.swaps.load(Ordering::Acquire),
        }
    }
}

/// この(オーディオ)スレッドでデノーマル数を 0 に丸める(FTZ / DAZ)。
/// 残響やフィルタが減衰しきる直前の極小値は x86 で桁違いに遅い演算になり、
/// 音の消え際で処理落ちを起こすことがある。DAW では標準的な対策。
#[inline]
pub fn flush_denormals() {
    #[cfg(target_arch = "x86_64")]
    #[allow(deprecated)]
    // SAFETY: MXCSR の FTZ(bit 15)と DAZ(bit 6)を立てるだけ。このスレッドの
    // 浮動小数演算の丸め方が変わるが、オーディオ処理では望ましい挙動
    unsafe {
        use std::arch::x86_64::{_mm_getcsr, _mm_setcsr};
        _mm_setcsr(_mm_getcsr() | 0x8040);
    }
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
            metronome: AtomicBool::new(false),
            recording: AtomicBool::new(false),
            click_only: AtomicBool::new(false),
            data: ArcSwap::from_pointee(data),
            stats: StatsCounters::default(),
            live: LiveQueue::default(),
            live_track: AtomicU32::new(LIVE_NO_TRACK),
            epoch: std::time::Instant::now(),
            block_frames: AtomicU32::new(0),
            pos_nanos: AtomicU64::new(0),
        }
    }

    /// いま聞こえている(と推定される)再生位置(サンプル、小数)。
    /// レンダラは 1 ブロック先まで書いてから `pos` を更新するので、1 ブロック分戻し、
    /// 前回の書き込みからの経過時間ぶん進める(1 ブロック以内に制限)。概算であり、
    /// デバイス固有の出力遅延までは含まない
    pub fn audible_pos(&self) -> f64 {
        let pos = self.pos.load(Ordering::Acquire) as f64;
        if !self.playing.load(Ordering::Acquire) {
            return pos;
        }
        let block = self.block_frames.load(Ordering::Acquire) as f64;
        let sr = self.data.load().sample_rate;
        let written = self.pos_nanos.load(Ordering::Acquire);
        let now = self.epoch.elapsed().as_nanos() as u64;
        let elapsed = now.saturating_sub(written) as f64 * 1e-9 * sr;
        (pos - block + elapsed.min(block)).max(0.0)
    }
}

#[derive(Clone, Copy)]
struct Voice {
    end: u64,
    track: u32,
    released: bool,
    /// ループ折り返しを何回またいだか。リリースの長い音が周回ごとに世代累積して
    /// ボイスプールを食い潰さないよう、2 回またいだら強制解放する
    wraps: u8,
    state: VoiceState,
}

/// 再生中の音声クリップ。波形は `data.audio_events[idx]` を参照する
/// (データ差し替え時は resync で作り直すので添字が古くなることはない)。
#[derive(Clone, Copy)]
struct AudioVoice {
    idx: usize,
    /// 波形内の再生位置(ネイティブレートのフレーム、小数)
    pos: f64,
}

/// メトロノームのクリック(減衰するサイン波。小節頭は高い音)。
#[derive(Clone, Copy, Default)]
struct Click {
    active: bool,
    age: u32,
    downbeat: bool,
}

impl Click {
    fn next(&mut self, sr: f32) -> f32 {
        if !self.active {
            return 0.0;
        }
        let t = self.age as f32 / sr;
        if t > 0.06 {
            self.active = false;
            return 0.0;
        }
        self.age += 1;
        let freq = if self.downbeat { 1500.0 } else { 1000.0 };
        let env = (-t / 0.012).exp();
        0.35 * env * (t * freq * std::f32::consts::TAU).sin()
    }
}

/// UI からの試聴用ボイス。停止中でも鳴り、トラックエフェクトは通さない。
/// instrument はサンプラーで Arc を含むため Copy ではない(clone は参照カウントのみ)。
#[derive(Clone)]
struct PreviewVoice {
    /// note_off までの残りサンプル数
    remaining: u32,
    released: bool,
    gain_l: f32,
    gain_r: f32,
    instrument: glaux_dsp::InstrumentParams,
    state: VoiceState,
}

/// MIDI キーボードのライブ発音ボイス。送り先トラックの楽器で鳴らし、
/// トラックのエフェクト・音量・パンを通す(停止中でも鳴る)。
#[derive(Clone)]
struct LiveVoice {
    /// 送り先トラック index(`LIVE_NO_TRACK` なら既定音色でマスター直行)
    track: u32,
    pitch: u8,
    released: bool,
    /// ペダルで保持中(ペダルを離したらリリース)
    sustained: bool,
    instrument: glaux_dsp::InstrumentParams,
    state: VoiceState,
}

pub struct Renderer {
    shared: Arc<Shared>,
    voices: Vec<Voice>,
    audio_voices: Vec<AudioVoice>,
    /// `data.audio_events` の次に開始するイベントの添字
    next_audio: usize,
    preview_voices: Vec<PreviewVoice>,
    /// 最後に消費した試聴要求のカウンタ
    last_preview: u64,
    live_voices: Vec<LiveVoice>,
    /// サステインペダルを踏んでいるか
    sustain: bool,
    /// ライブ演奏の残響を停止中にも鳴らす残りサンプル数
    live_tail: u32,
    /// エフェクト状態プール(リバーブのバッファ込みで起動時に確保)
    effect_states: Vec<EffectState>,
    /// トラックごとの無音連続サンプル数(残響が消えたらエフェクト処理を省く)
    track_silence: [u32; MAX_TRACKS],
    /// オートメーション評価カーソル(vol, pan)。単調前進、resync でリセット
    auto_cursors: [(usize, usize); MAX_TRACKS],
    /// マスター音量オートメーションの評価カーソル
    master_cursor: usize,
    /// device オートメーション適用済みの楽器パラメータ(トラック別スクラッチ)。
    /// レーンのあるトラックだけブロック頭でベースからコピーして値を上書きする。
    /// 起動時に確保し、以後アロケーションしない(clone は Arc 参照カウントのみ)
    inst_scratch: Vec<glaux_dsp::InstrumentParams>,
    /// fx オートメーション適用済みのエフェクト定義(状態スロット別)。None なら焼いたまま使う
    fx_scratch: Vec<Option<EffectParams>>,
    /// `data.events` の次に発音するイベントの添字
    next_event: usize,
    /// 直前に見ていた `PlaybackData` のアドレス(差し替え検出用)
    last_data: usize,
    pos: u64,
    /// 直前ブロックで再生中だったか(再開時に音声クリップを途中から鳴らし直す)
    was_playing: bool,
    /// メトロノーム: 次のクリック位置と発音中のクリック
    next_beat: Option<crate::data::Beat>,
    click: Click,
    /// `pos` に対応する音楽的位置(tick)。データ差し替え(テンポ変更)時に
    /// この tick を保ったままサンプル位置を換算し直す
    last_tick: f64,
    /// 前回のコールバック時刻(呼び出し遅延の検出用)
    last_call: Option<std::time::Instant>,
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
            audio_voices: Vec::with_capacity(MAX_AUDIO_VOICES),
            next_audio: 0,
            preview_voices: Vec::with_capacity(MAX_PREVIEW_VOICES),
            last_preview: 0,
            live_voices: Vec::with_capacity(MAX_LIVE_VOICES),
            sustain: false,
            live_tail: 0,
            effect_states: vec![EffectState::default(); MAX_EFFECT_SLOTS],
            track_silence: [u32::MAX; MAX_TRACKS],
            auto_cursors: [(0, 0); MAX_TRACKS],
            master_cursor: 0,
            inst_scratch: vec![glaux_dsp::InstrumentParams::default(); MAX_TRACKS],
            fx_scratch: vec![None; MAX_EFFECT_SLOTS],
            next_event: 0,
            last_data: 0,
            pos: 0,
            was_playing: false,
            next_beat: None,
            click: Click::default(),
            last_tick: 0.0,
            last_call: None,
        }
    }

    /// 音声クリップの再生状態を現在位置に合わせて作り直す。
    /// 位置をまたいでいるクリップは途中から鳴らす(シーク・ループ折返し・
    /// 編集によるデータ差し替えのどれでも音が途切れない)。
    fn resync_audio(&mut self, data: &PlaybackData) {
        self.audio_voices.clear();
        self.next_audio = data.audio_events.partition_point(|e| e.start < self.pos);
        for (idx, ev) in data.audio_events[..self.next_audio].iter().enumerate() {
            if ev.end <= self.pos || self.audio_voices.len() >= MAX_AUDIO_VOICES {
                continue;
            }
            if !data
                .tracks
                .get(ev.track as usize)
                .is_some_and(|m| m.audible)
            {
                continue;
            }
            self.audio_voices.push(AudioVoice {
                idx,
                pos: ev.offset + (self.pos - ev.start) as f64 * ev.rate,
            });
        }
    }

    /// `out` はインターリーブされた出力バッファ。
    pub fn process(&mut self, out: &mut [f32], channels: usize) {
        flush_denormals();
        let t0 = std::time::Instant::now();
        let was_playing = self.was_playing;
        self.process_inner(out, channels);

        // 負荷統計(ロック・アロケーションなし)
        let sr = self.shared.data.load().sample_rate.max(1.0);
        let frames = out.len() / channels.max(1);
        let budget = (frames as f64 / sr * 1e9) as u64;
        let busy = t0.elapsed().as_nanos() as u64;
        let st = &self.shared.stats;
        st.busy_ns.fetch_add(busy, Ordering::Relaxed);
        st.budget_ns.fetch_add(budget, Ordering::Relaxed);
        if let Some(permille) = (busy * 1000).checked_div(budget) {
            st.max_permille.fetch_max(permille, Ordering::Relaxed);
        }
        if busy > budget {
            st.overruns.fetch_add(1, Ordering::Relaxed);
        }
        if let Some(prev) = self.last_call {
            let gap = t0.duration_since(prev).as_nanos() as u64;
            if was_playing && self.was_playing && gap > budget * 18 / 10 {
                st.late.fetch_add(1, Ordering::Relaxed);
            }
        }
        self.last_call = Some(t0);
    }

    fn process_inner(&mut self, out: &mut [f32], channels: usize) {
        out.fill(0.0);
        if channels == 0 {
            return;
        }

        let guard = self.shared.data.load();
        let data: &PlaybackData = &guard;
        let data_addr = std::sync::Arc::as_ptr(&guard) as usize;

        // データ差し替え・シークのどちらでも発音状態を作り直す
        let mut resync = false;
        let mut swapped = false;
        if data_addr != self.last_data {
            swapped = self.last_data != 0;
            self.last_data = data_addr;
            resync = true;
        }
        let seek = self.shared.seek.swap(NO_SEEK, Ordering::AcqRel);
        if swapped && self.was_playing {
            self.shared.stats.swaps.fetch_add(1, Ordering::Relaxed);
        }
        if seek != NO_SEEK {
            self.pos = seek;
            resync = true;
        } else if swapped && !data.tempo.is_empty() {
            // テンポが変わっていても音楽的位置(tick)を保つ
            self.pos = data.tick_to_sample(self.last_tick);
        }
        if resync {
            self.voices.clear();
            self.next_beat = None; // シークで戻ったら拍を取り直す
            self.auto_cursors = [(0, 0); MAX_TRACKS];
            self.master_cursor = 0;
            self.next_event = data.events.partition_point(|e| e.start < self.pos);
            self.resync_audio(data);
            // 旧データの Arc(サンプル波形等)を掴んだままにしないようスクラッチを戻す
            for s in self.inst_scratch.iter_mut() {
                *s = glaux_dsp::InstrumentParams::default();
            }
            for s in self.fx_scratch.iter_mut() {
                *s = None;
            }
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
                Some(mix) => (mix.instrument.clone(), mix.gain_l, mix.gain_r),
                None => (glaux_dsp::InstrumentParams::default(), 0.8, 0.8),
            };
            if self.preview_voices.len() < MAX_PREVIEW_VOICES {
                let freq = crate::data::pitch_to_freq(pitch);
                let state = VoiceState::start(
                    &instrument,
                    freq,
                    pitch,
                    vel as f32 / 127.0,
                    glaux_core::Articulation::Normal,
                    sr,
                );
                self.preview_voices.push(PreviewVoice {
                    remaining: (dur_ms as f32 / 1000.0 * sr) as u32,
                    released: false,
                    gain_l,
                    gain_r,
                    instrument,
                    state,
                });
            }
        }

        // MIDI キーボードのライブ演奏(ブロック頭でまとめて反映。遅れは最大 1 ブロック)
        self.consume_live(data, sr);

        let playing = self.shared.playing.load(Ordering::Acquire);
        if playing && !self.was_playing && !resync {
            self.resync_audio(data);
        }
        self.was_playing = playing;
        if !playing {
            self.voices.clear();
            self.audio_voices.clear();
            if self.preview_voices.is_empty() && self.live_voices.is_empty() && self.live_tail == 0
            {
                self.last_tick = data.sample_to_tick(self.pos);
                self.publish_pos(out.len() / channels);
                return;
            }
        }
        let block_frames = out.len() / channels;
        if self.live_voices.is_empty() {
            self.live_tail = self.live_tail.saturating_sub(block_frames as u32);
        } else {
            self.live_tail = (LIVE_TAIL_SECS * sr) as u32;
        }

        let hard_limit = (VOICE_HARD_LIMIT_SECS * sr) as u64;
        let frames = out.len() / channels;

        // ループ区間(このブロックの間は固定値として扱う)
        let loop_start = self.shared.loop_start.load(Ordering::Acquire);
        let loop_end = self.shared.loop_end.load(Ordering::Acquire);
        let looping = loop_end > loop_start;

        // メトロノーム: このブロックで最初に来る拍を求める(resync 後も自然に追従)
        let metronome = self.shared.metronome.load(Ordering::Acquire);
        let click_only = self.shared.click_only.load(Ordering::Acquire);
        if metronome && playing {
            if self.next_beat.map_or(true, |b| b.sample < self.pos) {
                self.next_beat = data.next_beat(self.pos);
            }
        } else {
            self.next_beat = None;
        }

        // 音色パラメータのオートメーション: ブロック頭で評価してスクラッチに適用する
        // (1 ブロック ≈ 数 ms なので聴感上は連続。発音中のボイスにも効く =
        //  フィルタスイープ等が鳴る)
        // エフェクトのパラメータのオートメーションも同様(スロット別の作業用コピー)
        for mix in data.tracks.iter().take(MAX_TRACKS) {
            for (slot, name, points) in &mix.fx_auto {
                let Some(fx) = mix.effects.iter().find(|f| f.slot == *slot) else {
                    continue;
                };
                let s = *slot as usize;
                if s >= self.fx_scratch.len() {
                    continue;
                }
                let mut p = self.fx_scratch[s].unwrap_or(fx.params);
                let mut cursor = points.partition_point(|pt| pt.sample <= self.pos);
                cursor = cursor.saturating_sub(1);
                let v = eval_auto(points, &mut cursor, self.pos);
                p.set_continuous(name, v, sr);
                self.fx_scratch[s] = Some(p);
            }
        }
        for (slot, name, points) in &data.master_fx_auto {
            let Some(fx) = data.master_effects.iter().find(|f| f.slot == *slot) else {
                continue;
            };
            let s = *slot as usize;
            if s >= self.fx_scratch.len() {
                continue;
            }
            let mut p = self.fx_scratch[s].unwrap_or(fx.params);
            let cursor = points.partition_point(|pt| pt.sample <= self.pos);
            let v = eval_auto(points, &mut cursor.saturating_sub(1), self.pos);
            p.set_continuous(name, v, sr);
            self.fx_scratch[s] = Some(p);
        }
        for (ti, mix) in data.tracks.iter().take(MAX_TRACKS).enumerate() {
            if mix.device_auto.is_empty() {
                continue;
            }
            self.inst_scratch[ti] = mix.instrument.clone();
            for (name, points) in &mix.device_auto {
                let mut cursor = points.partition_point(|p| p.sample <= self.pos);
                cursor = cursor.saturating_sub(1);
                let v = eval_auto(points, &mut cursor, self.pos);
                self.inst_scratch[ti].set_continuous(name, v);
            }
        }

        for frame in 0..frames {
            // ループ終端に達したら区間頭へ。発音中の音は note_off でリリースに回し
            // (ぶつ切りのクリックを避ける)、イベント・オートメーションのカーソルを再同期。
            // 旧世代のボイスは 1 周分のリリース猶予の後に解放する(無限に世代が
            // 積み重なって CPU が漸増するのを防ぐ)
            if playing && looping && self.pos >= loop_end {
                self.pos = loop_start;
                self.voices.retain_mut(|v| {
                    if !v.released {
                        v.state.note_off();
                        v.released = true;
                    }
                    v.wraps = v.wraps.saturating_add(1);
                    v.wraps < 2
                });
                self.auto_cursors = [(0, 0); MAX_TRACKS];
                self.master_cursor = 0;
                self.next_event = data.events.partition_point(|e| e.start < self.pos);
                self.resync_audio(data);
                if metronome {
                    self.next_beat = data.next_beat(self.pos);
                }
            }

            // メトロノーム: 拍の位置でクリックを鳴らす
            if let Some(b) = self.next_beat {
                if playing && self.pos >= b.sample {
                    self.click = Click {
                        active: true,
                        age: 0,
                        downbeat: b.downbeat,
                    };
                    self.next_beat = data.next_beat(self.pos + 1);
                }
            }

            // このサンプル位置で始まる音声クリップを開始
            while playing
                && !click_only
                && self.next_audio < data.audio_events.len()
                && data.audio_events[self.next_audio].start <= self.pos
            {
                let idx = self.next_audio;
                self.next_audio += 1;
                let ev = &data.audio_events[idx];
                let audible = data
                    .tracks
                    .get(ev.track as usize)
                    .is_some_and(|m| m.audible);
                if audible && self.audio_voices.len() < MAX_AUDIO_VOICES {
                    self.audio_voices.push(AudioVoice {
                        idx,
                        pos: ev.offset,
                    });
                }
            }

            // このサンプル位置で始まるノートを発音(容量超過分は捨てる)
            while playing
                && !click_only
                && self.next_event < data.events.len()
                && data.events[self.next_event].start <= self.pos
            {
                let e = data.events[self.next_event];
                self.next_event += 1;
                let mix = data.tracks.get(e.track as usize);
                if let Some(mix) = mix.filter(|m| m.audible) {
                    if self.voices.len() < MAX_VOICES {
                        // 発音時パラメータ(pluck 等)にもスイープ中の値を反映する
                        let ti = e.track as usize;
                        let inst = if !mix.device_auto.is_empty() && ti < MAX_TRACKS {
                            &self.inst_scratch[ti]
                        } else {
                            &mix.instrument
                        };
                        let mut state =
                            VoiceState::start(inst, e.freq, e.pitch, e.amp, e.articulation, sr);
                        if !e.curve.is_empty() {
                            state.set_curve(&e.curve);
                        }
                        self.voices.push(Voice {
                            end: e.end,
                            track: e.track,
                            released: false,
                            wraps: 0,
                            state,
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
                // device オートメーションのあるトラックはスクラッチ(適用済み)を読む
                let ti = v.track as usize;
                let inst = if !mix.device_auto.is_empty() && ti < MAX_TRACKS {
                    &self.inst_scratch[ti]
                } else {
                    &mix.instrument
                };
                // 鳴り終わったボイスはノート終了を待たずに解放する
                // (減衰しきったピアノ・読み切ったワンショット等が
                //  スロットと CPU を占有し続けないように)
                if v.state.finished(inst) || self.pos >= v.end + hard_limit {
                    self.voices.swap_remove(i);
                    continue;
                }
                let sample = v.state.next(inst);
                match track_mono.get_mut(v.track as usize) {
                    Some(acc) => *acc += sample,
                    None => {
                        direct_l += sample * mix.gain_l;
                        direct_r += sample * mix.gain_r;
                    }
                }
                i += 1;
            }

            // 音声クリップ(線形補間で読み出し、フェードを掛けてトラックへ合算)
            let mut i = 0;
            while i < self.audio_voices.len() {
                let v = &mut self.audio_voices[i];
                let ev = &data.audio_events[v.idx];
                let frames = &ev.data.frames;
                let i0 = v.pos as usize;
                if self.pos >= ev.end || i0 + 1 >= frames.len() {
                    self.audio_voices.swap_remove(i);
                    continue;
                }
                let frac = (v.pos - i0 as f64) as f32;
                let mut sample = (frames[i0] + (frames[i0 + 1] - frames[i0]) * frac) * ev.gain;
                let since_start = self.pos - ev.start;
                if ev.fade_in > 0 && since_start < ev.fade_in {
                    sample *= since_start as f32 / ev.fade_in as f32;
                }
                let until_end = ev.end - self.pos;
                if ev.fade_out > 0 && until_end < ev.fade_out {
                    sample *= until_end as f32 / ev.fade_out as f32;
                }
                v.pos += ev.rate;
                match track_mono.get_mut(ev.track as usize) {
                    Some(acc) => *acc += sample,
                    None => {
                        if let Some(mix) = data.tracks.get(ev.track as usize) {
                            direct_l += sample * mix.gain_l;
                            direct_r += sample * mix.gain_r;
                        }
                    }
                }
                i += 1;
            }

            // MIDI キーボードのライブ発音(送り先トラックのエフェクトを通す)
            let mut i = 0;
            while i < self.live_voices.len() {
                let v = &mut self.live_voices[i];
                if v.released && v.state.finished(&v.instrument) {
                    self.live_voices.swap_remove(i);
                    continue;
                }
                let sample = v.state.next(&v.instrument);
                match data.tracks.get(v.track as usize) {
                    Some(mix) => match track_mono.get_mut(v.track as usize) {
                        Some(acc) => *acc += sample,
                        None => {
                            direct_l += sample * mix.gain_l;
                            direct_r += sample * mix.gain_r;
                        }
                    },
                    None => {
                        direct_l += sample * 0.8;
                        direct_r += sample * 0.8;
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
                    let params = self.fx_scratch[fx.slot as usize]
                        .as_ref()
                        .unwrap_or(&fx.params);
                    let key = sidechain_key(params);
                    let state = &mut self.effect_states[fx.slot as usize];
                    (fl, fr) = state.process(params, fl, fr, key);
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
                let params = self.fx_scratch[fx.slot as usize]
                    .as_ref()
                    .unwrap_or(&fx.params);
                let key = sidechain_key(params);
                let state = &mut self.effect_states[fx.slot as usize];
                (l, r) = state.process(params, l, r, key);
            }
            let master_amp = if data.master_vol_auto.is_empty() {
                data.master_amp
            } else {
                db_to_amp(eval_auto(
                    &data.master_vol_auto,
                    &mut self.master_cursor,
                    self.pos,
                ))
            };
            // クリックはマスターエフェクト・マスター音量を通さず直接足す
            let click = self.click.next(sr);
            let base = frame * channels;
            out[base] = (l * master_amp + click).tanh();
            if channels >= 2 {
                out[base + 1] = (r * master_amp + click).tanh();
            }
            if playing {
                self.pos += 1;
            }
        }

        // 曲が終わって余韻も消えたら自動停止(ループ中・録音中は止めない)
        let tail = (TAIL_SECS * data.sample_rate) as u64;
        let recording = self.shared.recording.load(Ordering::Acquire);
        if playing
            && !looping
            && !recording
            && data.end_sample > 0
            && self.pos > data.end_sample + tail
        {
            self.shared.playing.store(false, Ordering::Release);
        }

        self.last_tick = data.sample_to_tick(self.pos);
        self.publish_pos(frames);
    }

    /// 再生位置を UI・MIDI 受信側へ公開する(書いた時刻とブロック長も添える)。
    fn publish_pos(&self, frames: usize) {
        let sh = &self.shared;
        sh.pos.store(self.pos, Ordering::Release);
        sh.block_frames.store(frames as u32, Ordering::Release);
        sh.pos_nanos
            .store(sh.epoch.elapsed().as_nanos() as u64, Ordering::Release);
    }

    /// ライブ演奏キューを取り出して発音・消音する(アロケーションなし)。
    fn consume_live(&mut self, data: &PlaybackData, sr: f32) {
        for _ in 0..MAX_LIVE_EVENTS_PER_BLOCK {
            let Some(ev) = self.shared.live.pop() else {
                break;
            };
            match ev {
                LiveEvent::NoteOn { track, pitch, vel } => {
                    // 同じ音高を打ち直したら前の音はリリースへ
                    for v in self.live_voices.iter_mut() {
                        if v.pitch == pitch && !v.released {
                            v.state.note_off();
                            v.released = true;
                            v.sustained = false;
                        }
                    }
                    if self.live_voices.len() >= MAX_LIVE_VOICES {
                        // 満杯なら離した音を優先して(無ければ先頭を)捨てる
                        let victim = self
                            .live_voices
                            .iter()
                            .position(|v| v.released)
                            .unwrap_or(0);
                        self.live_voices.swap_remove(victim);
                    }
                    let (instrument, track) = match data.tracks.get(track as usize) {
                        Some(mix) if track != LIVE_NO_TRACK => (mix.instrument.clone(), track),
                        _ => (glaux_dsp::InstrumentParams::default(), LIVE_NO_TRACK),
                    };
                    let state = VoiceState::start(
                        &instrument,
                        crate::data::pitch_to_freq(pitch),
                        pitch,
                        vel as f32 / 127.0,
                        glaux_core::Articulation::Normal,
                        sr,
                    );
                    self.live_voices.push(LiveVoice {
                        track,
                        pitch,
                        released: false,
                        sustained: false,
                        instrument,
                        state,
                    });
                }
                LiveEvent::NoteOff { pitch } => {
                    for v in self.live_voices.iter_mut() {
                        if v.pitch == pitch && !v.released {
                            if self.sustain {
                                v.sustained = true;
                            } else {
                                v.state.note_off();
                                v.released = true;
                            }
                        }
                    }
                }
                LiveEvent::Sustain(on) => {
                    self.sustain = on;
                    if !on {
                        for v in self.live_voices.iter_mut() {
                            if v.sustained && !v.released {
                                v.state.note_off();
                                v.released = true;
                                v.sustained = false;
                            }
                        }
                    }
                }
                LiveEvent::AllOff => {
                    self.sustain = false;
                    for v in self.live_voices.iter_mut() {
                        if !v.released {
                            v.state.note_off();
                            v.released = true;
                            v.sustained = false;
                        }
                    }
                }
            }
        }
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
                curve: Default::default(),
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
                device_auto: vec![],
                fx_auto: vec![],
                instrument: test_instrument(),
                effects: vec![],
            }],
            audio_events: vec![],
            master_effects: vec![],
            master_amp: 1.0,
            master_vol_auto: vec![],
            master_fx_auto: vec![],
            end_sample: end,
            sample_rate: 48_000.0,
            tempo: vec![],
            sigs: vec![],
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
    fn loop_wraps_do_not_accumulate_released_voices() {
        // リリースの非常に長い音 + 短いループ。折り返しごとに旧世代を解放しないと
        // ボイスが際限なく積み重なって CPU が漸増する(実機で「段々カクつく」報告の原因)
        let mut data = data_with_note(0, 4700, true);
        if let InstrumentParams::Subtractive(p) = &mut data.tracks[0].instrument {
            p.release = 100.0;
            p.sustain = 1.0;
        }
        let shared = Arc::new(Shared::new(data));
        shared.playing.store(true, Ordering::Release);
        shared.loop_start.store(0, Ordering::Release);
        shared.loop_end.store(4800, Ordering::Release);
        let mut r = Renderer::new(shared.clone());

        for _ in 0..30 {
            let _ = render_block(&mut r, 4800); // 1 ブロック = ちょうど 1 周
        }
        assert!(
            r.voices.len() <= 3,
            "旧世代ボイスが解放されず {} 個残っている",
            r.voices.len()
        );
        // 音は出続けている(現行世代は生きている)
        let block = render_block(&mut r, 2400);
        assert!(rms(&block) > 0.05);
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
    fn finished_voices_are_released_before_note_end() {
        // ワンショット(ドラム)が鳴り終わったら、ノートが長くても
        // スロットを占有し続けない(SF2 ピアノ等の CPU 漸増対策の回帰テスト)
        let mut data = data_with_note(0, 480_000, true); // 10 秒のノート
        data.tracks[0].instrument = InstrumentParams::Drum(glaux_dsp::DrumParams {
            gain: 1.0,
            decay: 1.0,
            tone: 0.5,
            tune: 0.0,
        });
        data.events[0].pitch = 42; // クローズドハット(短い)
        let shared = Arc::new(Shared::new(data));
        shared.playing.store(true, Ordering::Release);
        let mut r = Renderer::new(shared.clone());

        let _ = render_block(&mut r, 512);
        assert_eq!(r.voices.len(), 1, "発音直後はボイスがあるはず");
        // 2 秒後(ハットは鳴り終わっている。ノートはまだ 8 秒残っている)
        for _ in 0..20 {
            let _ = render_block(&mut r, 4800);
        }
        assert_eq!(
            r.voices.len(),
            0,
            "鳴り終わったボイスはノート終了前に解放されるはず"
        );
    }

    #[test]
    fn metronome_clicks_on_beats_and_recording_blocks_auto_stop() {
        // 無音のデータ(ノートは可聴でない)+ 120bpm のテンポ表 → 拍位置でだけ音が出る
        let mut data = data_with_note(0, 1, false);
        data.tempo = vec![crate::data::TempoSeg {
            sample: 0,
            tick: 0,
            samples_per_tick: 25.0,
        }];
        data.sigs = vec![(0, 4, 4)];
        data.end_sample = 1;
        let shared = Arc::new(Shared::new(data));
        shared.playing.store(true, Ordering::Release);
        shared.metronome.store(true, Ordering::Release);
        shared.recording.store(true, Ordering::Release);
        let mut r = Renderer::new(shared.clone());
        // 0.5 秒 = 1 拍。拍頭直後 30ms は鳴り、拍の後半は無音
        let block = render_block(&mut r, 1440); // 0..30ms
        assert!(rms(&block) > 0.05, "拍頭でクリックが鳴るはず");
        let _ = render_block(&mut r, 24_000 - 1440 - 4800);
        let quiet = render_block(&mut r, 4800); // 拍の直前 100ms
        assert!(rms(&quiet) < 1e-4, "拍の間は無音");
        let next = render_block(&mut r, 1440); // 2 拍目の頭
        assert!(rms(&next) > 0.05, "次の拍でも鳴るはず");
        // end_sample + 余韻 2 秒を大きく超えても録音中は止まらない
        for _ in 0..40 {
            let _ = render_block(&mut r, 4800);
        }
        assert!(
            shared.playing.load(Ordering::Acquire),
            "録音中は自動停止しない"
        );
    }

    #[test]
    fn metronome_clicks_again_after_seeking_back() {
        let mut data = data_with_note(0, 1, false);
        data.tempo = vec![crate::data::TempoSeg {
            sample: 0,
            tick: 0,
            samples_per_tick: 25.0,
        }];
        data.sigs = vec![(0, 4, 4)];
        let shared = Arc::new(Shared::new(data));
        shared.playing.store(true, Ordering::Release);
        shared.metronome.store(true, Ordering::Release);
        let mut r = Renderer::new(shared.clone());
        // 1.5 拍ぶん進めてから先頭へ戻す → 先頭の拍がもう一度鳴る
        let _ = render_block(&mut r, 36_000);
        shared.seek.store(0, Ordering::Release);
        let block = render_block(&mut r, 1440);
        assert!(rms(&block) > 0.05, "戻った小節でもクリックが鳴るはず");
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
    fn live_midi_notes_sound_while_paused_and_release() {
        let shared = Arc::new(Shared::new(data_with_note(0, 4800, true)));
        let mut r = Renderer::new(shared.clone());
        assert!(rms(&render_block(&mut r, 4800)) < 1e-6, "何も無ければ無音");
        shared.live.push(LiveEvent::NoteOn {
            track: 0,
            pitch: 60,
            vel: 120,
        });
        let block = render_block(&mut r, 4800);
        assert!(rms(&block) > 0.05, "停止中でもライブ演奏は鳴るはず");
        // 押している間は鳴り続ける
        assert!(rms(&render_block(&mut r, 4800)) > 0.05);
        shared.live.push(LiveEvent::NoteOff { pitch: 60 });
        let _ = render_block(&mut r, 4800);
        assert!(rms(&render_block(&mut r, 4800)) < 1e-3, "離したら消える");
        assert_eq!(shared.pos.load(Ordering::Acquire), 0, "再生位置は動かない");
    }

    #[test]
    fn live_sustain_pedal_holds_until_released() {
        let shared = Arc::new(Shared::new(data_with_note(0, 4800, true)));
        let mut r = Renderer::new(shared.clone());
        shared.live.push(LiveEvent::Sustain(true));
        shared.live.push(LiveEvent::NoteOn {
            track: 0,
            pitch: 64,
            vel: 100,
        });
        let _ = render_block(&mut r, 480);
        shared.live.push(LiveEvent::NoteOff { pitch: 64 });
        let _ = render_block(&mut r, 4800);
        assert!(
            rms(&render_block(&mut r, 4800)) > 0.05,
            "ペダル中は鳴り続ける"
        );
        shared.live.push(LiveEvent::Sustain(false));
        let _ = render_block(&mut r, 4800);
        assert!(
            rms(&render_block(&mut r, 4800)) < 1e-3,
            "ペダルを離したら消える"
        );
    }

    #[test]
    fn live_voices_are_capped_and_all_off_silences() {
        let shared = Arc::new(Shared::new(data_with_note(0, 4800, true)));
        let mut r = Renderer::new(shared.clone());
        for p in 0..100u8 {
            shared.live.push(LiveEvent::NoteOn {
                track: LIVE_NO_TRACK,
                pitch: p,
                vel: 10,
            });
        }
        let _ = render_block(&mut r, 480);
        assert!(r.live_voices.len() <= MAX_LIVE_VOICES);
        assert!(r.live_voices.capacity() <= MAX_LIVE_VOICES, "再確保しない");
        shared.live.push(LiveEvent::AllOff);
        for _ in 0..8 {
            let _ = render_block(&mut r, 48_000); // 既定音色のリリースを待つ
        }
        assert!(r.live_voices.is_empty());
    }

    #[test]
    fn audible_pos_trails_written_pos_by_up_to_a_block() {
        let shared = Arc::new(Shared::new(data_with_note(0, 48_000, true)));
        shared.playing.store(true, Ordering::Release);
        let mut r = Renderer::new(shared.clone());
        let _ = render_block(&mut r, 960);
        let _ = render_block(&mut r, 960);
        let a = shared.audible_pos();
        assert!((960.0..=1920.0).contains(&a), "{a}");
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
