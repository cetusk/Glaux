//! `Project` からエンジン用の再生データを組み立てる。
//!
//! - **UI(非オーディオ)スレッドで**構築し、`ArcSwap` でオーディオスレッドに渡す。
//!   オーディオスレッドはこのデータを読むだけで、一切アロケーションしない。
//! - ノートは絶対サンプル位置に展開してソート済み。テンポは構築時に焼き込む
//!   (テンポ変更があればデータごと作り直す)。再生位置はサンプルで保持するが、
//!   テンポ区間表([`TempoSeg`])も焼き込んであり、レンダラは差し替え時に
//!   音楽的位置(tick)を保ったままサンプル位置を換算し直す。

use glaux_core::{AssetId, ClipContent, Curve, Effect, ParamPath, Project, Tick};
use glaux_dsp::{EffectParams, InstrumentParams, SampleData};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// エフェクト状態プールのスロット数(エンジン起動時に固定確保)。
/// これを超えたエフェクトは無視される(構築時に警告)。
pub const MAX_EFFECT_SLOTS: usize = 64;
/// エフェクト・ミックス処理の対象になる最大トラック数。
/// 超過分のトラックはエフェクトなしで直接マスターへ送られる。
pub const MAX_TRACKS: usize = 64;

/// 焼き込み済みエフェクト + 状態プールのスロット番号。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BakedEffect {
    pub params: EffectParams,
    pub slot: u32,
}

/// サンプル位置に焼き込んだオートメーション点。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AutoPoint {
    pub sample: u64,
    pub value: f32,
    pub curve: Curve,
}

/// 1 ノート分の再生イベント。サンプル位置は曲頭からの絶対値。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NoteEvent {
    pub start: u64,
    pub end: u64,
    /// 周波数(Hz)。ピッチから変換済み
    pub freq: f32,
    /// MIDI ノート番号(ドラムシンセの音色選択に使う)
    pub pitch: u8,
    /// ベロシティ由来の振幅 0.0..=1.0
    pub amp: f32,
    /// `PlaybackData::tracks` への添字
    pub track: u32,
    /// 奏法(パームミュート等)。ボイス起動時に音色へ反映する
    pub articulation: glaux_core::Articulation,
    /// 連続ピッチカーブ(ノート先頭からのサンプル数, セント)。空なら無し
    pub curve: glaux_dsp::PitchCurve,
}

/// 音声クリップ 1 つ分の再生イベント。サンプル位置は曲頭からの絶対値。
#[derive(Clone, Debug)]
pub struct AudioEvent {
    pub start: u64,
    pub end: u64,
    /// `PlaybackData::tracks` への添字
    pub track: u32,
    /// モノラル化済みの波形(ネイティブレート)
    pub data: Arc<SampleData>,
    /// 波形内の再生開始位置(ネイティブレートのフレーム)
    pub offset: f64,
    /// 1 出力サンプルあたりの進み(ネイティブレート / エンジンレート)
    pub rate: f64,
    /// リニアゲイン(clip.gain_db 由来)
    pub gain: f32,
    /// フェードイン / アウト(出力サンプル数)
    pub fade_in: u64,
    pub fade_out: u64,
}

impl PartialEq for AudioEvent {
    fn eq(&self, other: &Self) -> bool {
        self.start == other.start
            && self.end == other.end
            && self.track == other.track
            && Arc::ptr_eq(&self.data, &other.data)
            && self.offset == other.offset
    }
}

/// トラックのミックス設定(音量・パン・mute/solo を反映済み)。
#[derive(Clone, Debug, PartialEq)]
pub struct TrackMix {
    pub gain_l: f32,
    pub gain_r: f32,
    /// mute / solo 判定の結果。false なら発音しない
    pub audible: bool,
    /// 静的な音量(リニア)とパン。オートメーションと組み合わせるときに使う
    pub base_amp: f32,
    pub base_pan: f32,
    /// track/volume_db のオートメーション(dB 値)。空ならフェーダー値を使う。
    /// レーンがあるときは**フェーダーより優先**(一般的な DAW と同じ)
    pub vol_db_auto: Vec<AutoPoint>,
    /// track/pan のオートメーション(-1..1)。空ならフェーダー値を使う
    pub pan_auto: Vec<AutoPoint>,
    /// device/<param> のオートメーション(raw 値)。ブロックレートで
    /// `InstrumentParams::set_continuous` に流し込む(フィルタスイープ等)
    pub device_auto: Vec<(String, Vec<AutoPoint>)>,
    /// fx/<id>/<param> のオートメーション(エフェクトの状態スロット, パラメータ名, 点列)。
    /// ブロックレートで `EffectParams::set_continuous` に流し込む
    pub fx_auto: Vec<(u32, String, Vec<AutoPoint>)>,
    /// 焼き込み済みの楽器パラメータ(glaux-dsp)
    pub instrument: InstrumentParams,
    /// エフェクトチェーン(bypass 除外・焼き込み済み)。楽器 → チェーン → 音量/パン の順
    pub effects: Vec<BakedEffect>,
}

/// テンポ区間(サンプル位置 ⇔ tick の相互変換用)。`sample` 昇順。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TempoSeg {
    pub sample: u64,
    pub tick: u64,
    pub samples_per_tick: f64,
}

/// オーディオスレッドが読む再生データ一式。イミュータブル。
#[derive(Clone, Debug, Default)]
pub struct PlaybackData {
    /// `start` 昇順
    pub events: Vec<NoteEvent>,
    /// 音声クリップ。`start` 昇順
    pub audio_events: Vec<AudioEvent>,
    pub tracks: Vec<TrackMix>,
    /// マスターバスのエフェクトチェーン
    pub master_effects: Vec<BakedEffect>,
    pub master_amp: f32,
    /// 最後のノートが終わるサンプル位置(自動停止に使う)
    pub end_sample: u64,
    pub sample_rate: f64,
    /// テンポマップの焼き込み。データ差し替え時に「音楽的位置(tick)」を
    /// 保ったままサンプル位置を換算し直すために使う(空なら換算しない)
    pub tempo: Vec<TempoSeg>,
    /// 拍子イベント(tick, 分子, 分母)。メトロノームの拍・小節頭の判定に使う
    pub sigs: Vec<(u64, u8, u8)>,
}

/// メトロノームの次のクリック。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Beat {
    pub sample: u64,
    pub downbeat: bool,
}

impl PlaybackData {
    /// サンプル位置 → tick(小数)。区分線形。
    pub fn sample_to_tick(&self, sample: u64) -> f64 {
        let idx = self.tempo.partition_point(|s| s.sample <= sample);
        let Some(seg) = idx.checked_sub(1).and_then(|i| self.tempo.get(i)) else {
            return 0.0;
        };
        seg.tick as f64 + (sample - seg.sample) as f64 / seg.samples_per_tick
    }

    /// `pos`(サンプル)以降で最初に来る拍。テンポ・拍子が無ければ None。
    pub fn next_beat(&self, pos: u64) -> Option<Beat> {
        if self.tempo.is_empty() {
            return None;
        }
        let tick = self.sample_to_tick(pos);
        let (sig_tick, num, den) = self
            .sigs
            .iter()
            .rev()
            .find(|(t, _, _)| (*t as f64) <= tick)
            .copied()
            .or_else(|| self.sigs.first().copied())
            .unwrap_or((0, 4, 4));
        let beat_len = (3840.0 / den.max(1) as f64).max(1.0);
        let idx = ((tick - sig_tick as f64) / beat_len).floor().max(0.0);
        // 今の拍がちょうど pos 以上なら今の拍、そうでなければ次の拍
        for k in 0..2 {
            let i = idx + k as f64;
            let beat_tick = sig_tick as f64 + i * beat_len;
            let sample = self.tick_to_sample(beat_tick);
            if sample >= pos {
                return Some(Beat {
                    sample,
                    downbeat: (i as u64) % num.max(1) as u64 == 0,
                });
            }
        }
        None
    }

    /// tick(小数)→ サンプル位置。
    pub fn tick_to_sample(&self, tick: f64) -> u64 {
        let idx = self.tempo.partition_point(|s| (s.tick as f64) <= tick);
        let Some(seg) = idx.checked_sub(1).and_then(|i| self.tempo.get(i)) else {
            return 0;
        };
        seg.sample + ((tick - seg.tick as f64) * seg.samples_per_tick).round() as u64
    }
}

/// 読み込み済みサンプルの置き場(サンプラー音源用)。
/// UI(非オーディオ)スレッドで構築し、`Arc` で `PlaybackData` に焼き込む。
/// 編集のたびに WAV をデコードし直さないよう、アセット ID ごとにキャッシュする。
pub struct SampleBank {
    map: HashMap<AssetId, Arc<SampleData>>,
    /// SoundFont ライブラリフォルダ(既定は `sf2::default_dir()`)
    sf2_dir: PathBuf,
    /// パース済み SoundFont(ファイル名 → フォント)
    fonts: HashMap<String, Arc<rustysynth::SoundFont>>,
    /// 構築済みゾーン列((ファイル名, bank, preset) → zones)
    multis: HashMap<(String, u16, u16), Arc<Vec<glaux_dsp::Zone>>>,
}

impl Default for SampleBank {
    fn default() -> Self {
        SampleBank {
            map: HashMap::new(),
            sf2_dir: crate::sf2::default_dir(),
            fonts: HashMap::new(),
            multis: HashMap::new(),
        }
    }
}

impl SampleBank {
    pub fn get(&self, id: &AssetId) -> Option<&Arc<SampleData>> {
        self.map.get(id)
    }

    /// テスト・特殊環境用: SoundFont ライブラリフォルダを差し替える。
    pub fn with_sf2_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.sf2_dir = dir.into();
        self
    }

    pub fn get_multi(
        &self,
        soundfont: &str,
        bank: u16,
        preset: u16,
    ) -> Option<&Arc<Vec<glaux_dsp::Zone>>> {
        self.multis.get(&(soundfont.to_owned(), bank, preset))
    }

    /// プロジェクトの全アセットと SoundFont プリセットを読み込む
    /// (読み込み済みは再利用、使われなくなったものは破棄)。
    pub fn sync(&mut self, project: &Project, project_dir: &Path) {
        self.map.retain(|id, _| project.assets.contains_key(id));
        for (id, asset) in &project.assets {
            if self.map.contains_key(id) {
                continue;
            }
            match load_wav_mono(&project_dir.join(&asset.path)) {
                Ok(data) => {
                    self.map.insert(id.clone(), Arc::new(data));
                }
                Err(e) => {
                    tracing::warn!("サンプルを読み込めません({}): {e}", asset.path);
                }
            }
        }

        // SoundFont: プロジェクトが参照しているプリセットのゾーンを構築
        let mut used: std::collections::HashSet<(String, u16, u16)> =
            std::collections::HashSet::new();
        for t in &project.tracks {
            if let Some(d) = &t.device {
                if let glaux_core::PluginSource::Sf2 {
                    soundfont,
                    bank,
                    preset,
                } = &d.source
                {
                    used.insert((soundfont.clone(), *bank, *preset));
                }
            }
        }
        self.multis.retain(|k, _| used.contains(k));
        let used_fonts: std::collections::HashSet<&String> =
            used.iter().map(|(f, _, _)| f).collect();
        self.fonts.retain(|f, _| used_fonts.contains(f));
        for (file, bank, preset) in used {
            if self.multis.contains_key(&(file.clone(), bank, preset)) {
                continue;
            }
            let font = match self.fonts.get(&file) {
                Some(f) => f.clone(),
                None => match crate::sf2::load_font(&self.sf2_dir.join(&file)) {
                    Ok(f) => {
                        self.fonts.insert(file.clone(), f.clone());
                        f
                    }
                    Err(e) => {
                        tracing::warn!("{e}");
                        continue;
                    }
                },
            };
            match crate::sf2::build_zones(&font, bank, preset) {
                Some(zones) => {
                    self.multis.insert((file.clone(), bank, preset), zones);
                }
                None => {
                    tracing::warn!(
                        "SoundFont にプリセットがありません: {file} bank={bank} preset={preset}"
                    );
                }
            }
        }
    }

    /// 使い捨て(オフラインレンダ・解析用)に全アセットを読み込む。
    pub fn load(project: &Project, project_dir: &Path) -> SampleBank {
        let mut bank = SampleBank::default();
        bank.sync(project, project_dir);
        bank
    }
}

/// 波形の縮小表示用ピーク列: 区間ごとの (最小, 最大)。`buckets` 個に分ける。
pub fn wave_peaks(frames: &[f32], buckets: usize) -> Vec<(f32, f32)> {
    if frames.is_empty() || buckets == 0 {
        return vec![];
    }
    let per = frames.len() as f64 / buckets as f64;
    (0..buckets)
        .map(|b| {
            let s = (b as f64 * per) as usize;
            let e = (((b + 1) as f64 * per) as usize).clamp(s + 1, frames.len());
            frames[s..e]
                .iter()
                .fold((f32::MAX, f32::MIN), |(lo, hi), &v| (lo.min(v), hi.max(v)))
        })
        .collect()
}

/// WAV をモノラル f32 に読み込む(ステレオは平均で合算)。
pub fn load_wav_mono(path: &Path) -> Result<SampleData, String> {
    let mut reader = hound::WavReader::open(path).map_err(|e| e.to_string())?;
    let spec = reader.spec();
    let channels = spec.channels.max(1) as usize;
    let raw: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Float => reader
            .samples::<f32>()
            .collect::<Result<_, _>>()
            .map_err(|e| e.to_string())?,
        hound::SampleFormat::Int => {
            let scale = 1.0 / (1i64 << (spec.bits_per_sample - 1)) as f32;
            reader
                .samples::<i32>()
                .map(|s| s.map(|v| v as f32 * scale))
                .collect::<Result<_, _>>()
                .map_err(|e| e.to_string())?
        }
    };
    let frames = raw
        .chunks_exact(channels)
        .map(|c| c.iter().sum::<f32>() / channels as f32)
        .collect();
    Ok(SampleData {
        frames,
        sample_rate: spec.sample_rate as f32,
    })
}

/// トラックの音源を焼き込む(サンプラーは SampleBank から波形を解決)。
fn bake_track_instrument(
    t: &glaux_core::Track,
    bank: &SampleBank,
    sample_rate: f32,
) -> InstrumentParams {
    if let Some(d) = &t.device {
        match &d.source {
            glaux_core::PluginSource::Sampler { asset } => {
                if let Some(data) = bank.get(asset) {
                    return InstrumentParams::Sampler(glaux_dsp::bake_sampler(
                        &d.params,
                        data.clone(),
                        sample_rate,
                    ));
                }
                tracing::warn!("サンプル未読込のため subtractive で代用: {asset}");
            }
            glaux_core::PluginSource::Sf2 {
                soundfont,
                bank: b,
                preset,
            } => {
                if let Some(zones) = bank.get_multi(soundfont, *b, *preset) {
                    return InstrumentParams::Sf2(glaux_dsp::bake_sf2(&d.params, zones.clone()));
                }
                tracing::warn!("SoundFont 未読込のため subtractive で代用: {soundfont}");
            }
            _ => {}
        }
    }
    glaux_dsp::bake_instrument(t.device.as_ref()).1
}

pub fn db_to_amp(db: f32) -> f32 {
    10.0_f32.powf(db / 20.0)
}

pub fn pitch_to_freq(pitch: u8) -> f32 {
    440.0 * 2.0_f32.powf((pitch as f32 - 69.0) / 12.0)
}

/// 等パワーパン。pan は -1.0(L) ..= 1.0(R)。
pub fn pan_gains(pan: f32) -> (f32, f32) {
    let t = (pan.clamp(-1.0, 1.0) + 1.0) * std::f32::consts::FRAC_PI_4;
    (t.cos(), t.sin())
}

/// エフェクトチェーンを焼き込み、状態プールのスロットを割り当てる。
fn bake_chain(
    effects: &[Effect],
    sample_rate: f32,
    next_slot: &mut u32,
    resolve_track: &dyn Fn(&str) -> Option<u32>,
) -> Vec<BakedEffect> {
    bake_chain_with_ids(effects, sample_rate, next_slot, resolve_track)
        .into_iter()
        .map(|(_, b)| b)
        .collect()
}

/// `bake_chain` と同じだが、各エフェクトの ID も返す(オートメーションのスロット解決用)。
fn bake_chain_with_ids(
    effects: &[Effect],
    sample_rate: f32,
    next_slot: &mut u32,
    resolve_track: &dyn Fn(&str) -> Option<u32>,
) -> Vec<(glaux_core::FxId, BakedEffect)> {
    effects
        .iter()
        .filter(|e| !e.bypass)
        .filter_map(|e| {
            let params = glaux_dsp::bake_effect(e, sample_rate, resolve_track)?;
            if *next_slot as usize >= MAX_EFFECT_SLOTS {
                tracing::warn!(
                    "エフェクトが多すぎます({MAX_EFFECT_SLOTS} 超)。{} を無視",
                    e.id
                );
                return None;
            }
            let slot = *next_slot;
            *next_slot += 1;
            Some((e.id.clone(), BakedEffect { params, slot }))
        })
        .collect()
}

/// プロジェクト全体を再生データに展開する。
pub fn build_playback_data(project: &Project, sample_rate: f64, bank: &SampleBank) -> PlaybackData {
    let any_solo = project.tracks.iter().any(|t| t.solo);
    let to_sample =
        |tick: Tick| -> u64 { (project.tempo_map.tick_to_seconds(tick) * sample_rate) as u64 };

    // サイドチェインの source(トラック ID)→ index 解決
    let resolve_track = |id: &str| -> Option<u32> {
        project
            .tracks
            .iter()
            .position(|t| t.id.as_str() == id)
            .map(|i| i as u32)
    };

    // track/volume_db・track/pan のレーンをサンプル位置に焼き込む
    let bake_lane = |t: &glaux_core::Track, name: &str| -> Vec<AutoPoint> {
        let mut points: Vec<AutoPoint> = t
            .automation
            .iter()
            .find(|lane| matches!(&lane.target, ParamPath::Track { name: n } if n == name))
            .map(|lane| {
                lane.points
                    .iter()
                    .map(|p| AutoPoint {
                        sample: to_sample(p.tick),
                        value: p.value as f32,
                        curve: p.curve,
                    })
                    .collect()
            })
            .unwrap_or_default();
        points.sort_by_key(|p| p.sample);
        points
    };

    // device/<param> のレーンをまとめて焼き込む(値は raw のまま。適用は再生時)
    let bake_device_lanes = |t: &glaux_core::Track| -> Vec<(String, Vec<AutoPoint>)> {
        t.automation
            .iter()
            .filter_map(|lane| {
                let ParamPath::Device { name } = &lane.target else {
                    return None;
                };
                if lane.points.is_empty() {
                    return None;
                }
                let mut points: Vec<AutoPoint> = lane
                    .points
                    .iter()
                    .map(|p| AutoPoint {
                        sample: to_sample(p.tick),
                        value: p.value as f32,
                        curve: p.curve,
                    })
                    .collect();
                points.sort_by_key(|p| p.sample);
                Some((name.clone(), points))
            })
            .collect()
    };

    let mut next_slot: u32 = 0;
    let tracks: Vec<TrackMix> = project
        .tracks
        .iter()
        .map(|t| {
            let (pl, pr) = pan_gains(t.pan);
            let gain = db_to_amp(t.volume_db);
            let instrument = bake_track_instrument(t, bank, sample_rate as f32);
            let chain = bake_chain_with_ids(
                &t.effects,
                sample_rate as f32,
                &mut next_slot,
                &resolve_track,
            );
            // fx/<id>/<param> のレーンを、焼いたエフェクトのスロットに解決する
            // (バイパス中・未知のエフェクトのレーンは鳴らさない)
            let fx_auto = t
                .automation
                .iter()
                .filter_map(|lane| {
                    let ParamPath::Effect { id, name } = &lane.target else {
                        return None;
                    };
                    let (_, baked) = chain.iter().find(|(fid, _)| fid == id)?;
                    if lane.points.is_empty() {
                        return None;
                    }
                    let mut points: Vec<AutoPoint> = lane
                        .points
                        .iter()
                        .map(|p| AutoPoint {
                            sample: to_sample(p.tick),
                            value: p.value as f32,
                            curve: p.curve,
                        })
                        .collect();
                    points.sort_by_key(|p| p.sample);
                    Some((baked.slot, name.clone(), points))
                })
                .collect();
            TrackMix {
                gain_l: gain * pl,
                gain_r: gain * pr,
                audible: !t.mute && (!any_solo || t.solo),
                base_amp: gain,
                base_pan: t.pan,
                vol_db_auto: bake_lane(t, "volume_db"),
                pan_auto: bake_lane(t, "pan"),
                device_auto: bake_device_lanes(t),
                fx_auto,
                instrument,
                effects: chain.into_iter().map(|(_, b)| b).collect(),
            }
        })
        .collect();
    let master_effects = bake_chain(
        &project.master.effects,
        sample_rate as f32,
        &mut next_slot,
        &resolve_track,
    );

    let mut events = Vec::new();
    let mut audio_events = Vec::new();
    for (ti, track) in project.tracks.iter().enumerate() {
        for clip in &track.clips {
            let ClipContent::Midi { .. } = &clip.content else {
                if let ClipContent::Audio {
                    asset,
                    offset_samples,
                    gain_db,
                    fade_in_ms,
                    fade_out_ms,
                    ..
                } = &clip.content
                {
                    let Some(data) = bank.get(asset) else {
                        tracing::warn!("音声クリップの波形が未読込のため鳴らしません: {asset}");
                        continue;
                    };
                    let start = to_sample(clip.start);
                    let end = to_sample(clip.start + clip.length).max(start + 1);
                    audio_events.push(AudioEvent {
                        start,
                        end,
                        track: ti as u32,
                        data: data.clone(),
                        offset: *offset_samples as f64,
                        rate: data.sample_rate as f64 / sample_rate,
                        gain: db_to_amp(*gain_db),
                        fade_in: (*fade_in_ms as f64 * 0.001 * sample_rate) as u64,
                        fade_out: (*fade_out_ms as f64 * 0.001 * sample_rate) as u64,
                    });
                }
                continue;
            };
            // クリップ外の切り捨て・ループの繰り返し展開は core の playback_notes に任せる
            for note in &clip.playback_notes() {
                let dur = note.dur;
                let start_tick = clip.start + note.pos;
                let start = to_sample(start_tick);
                let mut end = to_sample(start_tick + dur).max(start + 1);
                // スタッカートは音価の半分で切る(歯切れの表現)
                if note.articulation == glaux_core::Articulation::Staccato {
                    end = start + ((end - start) / 2).max(1);
                }
                // ピッチカーブ: 相対 tick → ノート先頭からのサンプル数(テンポ考慮)
                let curve_pts: Vec<(f32, f32)> = note
                    .pitch_curve
                    .iter()
                    .map(|p| {
                        let at = to_sample(start_tick + p.tick).saturating_sub(start) as f32;
                        (at, p.cents)
                    })
                    .collect();
                events.push(NoteEvent {
                    start,
                    end,
                    freq: pitch_to_freq(note.pitch),
                    pitch: note.pitch,
                    amp: note.vel as f32 / 127.0,
                    track: ti as u32,
                    articulation: note.articulation,
                    curve: glaux_dsp::PitchCurve::from_points(&curve_pts),
                });
            }
        }
    }
    events.sort_by_key(|e| e.start);
    audio_events.sort_by_key(|e| e.start);
    let end_sample = events
        .iter()
        .map(|e| e.end)
        .chain(audio_events.iter().map(|e| e.end))
        .max()
        .unwrap_or(0);

    let tempo = project
        .tempo_map
        .events()
        .iter()
        .map(|ev| TempoSeg {
            sample: to_sample(ev.tick),
            tick: ev.tick.0,
            samples_per_tick: sample_rate * 60.0 / (ev.bpm * project.ppq as f64),
        })
        .collect();

    let sigs = project
        .time_sig_map
        .iter()
        .map(|e| (e.tick.0, e.num, e.den))
        .collect();

    PlaybackData {
        events,
        audio_events,
        tracks,
        master_effects,
        master_amp: db_to_amp(project.master.volume_db),
        end_sample,
        sample_rate,
        tempo,
        sigs,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use glaux_core::{Clip, ClipId, NoteId, Track, TrackId, TrackKind};

    fn note(pos: u64, dur: u64, pitch: u8, vel: u8) -> glaux_core::Note {
        glaux_core::Note {
            articulation: Default::default(),
            pitch_curve: vec![],
            id: NoteId::new(),
            pos: Tick(pos),
            dur: Tick(dur),
            pitch,
            vel,
        }
    }

    fn project_with_notes(notes: Vec<glaux_core::Note>) -> Project {
        let mut project = Project::new("t");
        let mut track = Track::new(TrackId::new(), "T1", TrackKind::Midi);
        let mut clip = Clip::new_midi(ClipId::new(), "c", Tick(0), Tick(3840));
        if let ClipContent::Midi { notes: n, .. } = &mut clip.content {
            *n = notes;
        }
        track.clips.push(clip);
        project.tracks.push(track);
        project
    }

    #[test]
    fn expands_notes_to_samples_at_120bpm() {
        // 120bpm: 960 tick = 0.5s = 24000 samples @48k
        let project = project_with_notes(vec![note(960, 960, 69, 127)]);
        let data = build_playback_data(&project, 48_000.0, &SampleBank::default());
        assert_eq!(data.events.len(), 1);
        let e = &data.events[0];
        assert_eq!(e.start, 24_000);
        assert_eq!(e.end, 48_000);
        assert!((e.freq - 440.0).abs() < 1e-3);
        assert!((e.amp - 1.0).abs() < 1e-6);
        assert_eq!(data.end_sample, 48_000);
    }

    #[test]
    fn loop_clip_repeats_in_playback() {
        use glaux_core::ClipContent;
        // 1 拍ぶんのパターン(頭に 1 音)を 1 小節のクリップでループ → 4 回鳴る
        let mut project = project_with_notes(vec![note(0, 240, 60, 100)]);
        if let ClipContent::Midi {
            looped, loop_len, ..
        } = &mut project.tracks[0].clips[0].content
        {
            *looped = true;
            *loop_len = Some(Tick(960));
        }
        let data = build_playback_data(&project, 48_000.0, &SampleBank::default());
        let starts: Vec<u64> = data.events.iter().map(|e| e.start).collect();
        // 120bpm: 960 tick = 24000 サンプル。クリップ長 3840 の中で 4 回
        assert_eq!(starts, vec![0, 24_000, 48_000, 72_000]);
    }

    #[test]
    fn staccato_halves_note_length() {
        let mut n = note(960, 960, 69, 127);
        n.articulation = glaux_core::Articulation::Staccato;
        let data = build_playback_data(&project_with_notes(vec![n]), 48_000.0, &Default::default());
        let e = &data.events[0];
        assert_eq!(e.start, 24_000);
        assert_eq!(e.end, 36_000, "音価の半分で切れるはず");
        assert_eq!(e.articulation, glaux_core::Articulation::Staccato);
    }

    #[test]
    fn truncates_notes_at_clip_end_and_sorts() {
        let project = project_with_notes(vec![
            note(3000, 5000, 60, 100), // クリップ長 3840 で切り詰め
            note(0, 480, 64, 100),
        ]);
        let data = build_playback_data(&project, 48_000.0, &SampleBank::default());
        assert_eq!(data.events.len(), 2);
        assert!(data.events[0].start <= data.events[1].start);
        // 3840 tick = 2.0s = 96000 samples が終端
        assert_eq!(data.end_sample, 96_000);
    }

    #[test]
    fn mute_and_solo_control_audibility() {
        let mut project = project_with_notes(vec![note(0, 480, 60, 100)]);
        let mut t2 = Track::new(TrackId::new(), "T2", TrackKind::Midi);
        t2.solo = true;
        project.tracks.push(t2);

        let data = build_playback_data(&project, 48_000.0, &SampleBank::default());
        // T2 が solo なので T1 は聞こえない
        assert!(!data.tracks[0].audible);
        assert!(data.tracks[1].audible);

        project.tracks[1].solo = false;
        project.tracks[0].mute = true;
        let data = build_playback_data(&project, 48_000.0, &SampleBank::default());
        assert!(!data.tracks[0].audible);
        assert!(data.tracks[1].audible);
    }

    #[test]
    fn bakes_effect_chains_with_slots() {
        use glaux_core::{Effect, FxId};
        let mut project = project_with_notes(vec![note(0, 480, 60, 100)]);
        let mut rv = Effect::builtin(FxId::new(), "reverb");
        rv.params.insert("mix".into(), 0.5.into());
        let mut bypassed = Effect::builtin(FxId::new(), "eq");
        bypassed.bypass = true;
        project.tracks[0].effects = vec![rv, bypassed];
        project
            .master
            .effects
            .push(Effect::builtin(FxId::new(), "compressor"));

        let data = build_playback_data(&project, 48_000.0, &SampleBank::default());
        // bypass は除外され、スロットは通しで振られる
        assert_eq!(data.tracks[0].effects.len(), 1);
        assert_eq!(data.tracks[0].effects[0].slot, 0);
        assert_eq!(data.master_effects.len(), 1);
        assert_eq!(data.master_effects[0].slot, 1);
    }

    #[test]
    fn reverb_effect_extends_render_tail() {
        use crate::export::render_project;
        use glaux_core::{Effect, FxId};
        let dry_project = project_with_notes(vec![note(0, 480, 72, 100)]);
        let mut wet_project = dry_project.clone();
        let mut rv = Effect::builtin(FxId::new(), "reverb");
        rv.params.insert("mix".into(), 0.6.into());
        rv.params.insert("size".into(), 0.9.into());
        wet_project.tracks[0].effects.push(rv);

        let dry = render_project(&dry_project, 48_000.0, &Default::default()).unwrap();
        let wet = render_project(&wet_project, 48_000.0, &Default::default()).unwrap();
        assert!(
            wet.len() > dry.len(),
            "残響で音の長さが伸びるはず({} vs {})",
            wet.len(),
            dry.len()
        );
    }

    #[test]
    fn volume_automation_fades_and_overrides_fader() {
        use crate::export::render_project;
        use glaux_core::{AutomationLane, AutomationPoint, Curve, ParamPath};

        // フェーダーは -60dB(ほぼ無音)だが、レーンが -60 → 0dB のフェードインを描く
        let mut project = project_with_notes(vec![note(0, 3840, 69, 127)]);
        project.tracks[0].volume_db = -60.0;
        project.tracks[0].automation.push(AutomationLane {
            target: ParamPath::track("volume_db"),
            points: vec![
                AutomationPoint {
                    tick: Tick(0),
                    value: -60.0,
                    curve: Curve::Linear,
                },
                AutomationPoint {
                    tick: Tick(1920), // 1 秒でフェード完了、残り 1 秒は 0dB
                    value: 0.0,
                    curve: Curve::Linear,
                },
            ],
        });
        let out = render_project(&project, 48_000.0, &Default::default()).unwrap();
        let rms = |sl: &[f32]| (sl.iter().map(|s| s * s).sum::<f32>() / sl.len() as f32).sqrt();
        // ステレオ interleaved: 秒 → サンプル対
        let sec = |a: f64, b: f64| &out[(a * 96_000.0) as usize..(b * 96_000.0) as usize];
        let head = rms(sec(0.0, 0.4));
        let tail = rms(sec(1.2, 1.8)); // フェード完了後・ノート内
        assert!(
            tail > head * 4.0,
            "フェードインするはず: head={head} tail={tail}"
        );
        assert!(
            tail > 0.06,
            "レーンがフェーダー(-60dB)より優先されるはず: {tail}"
        );
    }

    #[test]
    fn device_automation_lane_is_baked() {
        use glaux_core::{AutomationLane, AutomationPoint, Curve, ParamPath};
        let mut project = project_with_notes(vec![note(0, 480, 60, 100)]);
        project.tracks[0].automation.push(AutomationLane {
            target: ParamPath::device("cutoff"),
            points: vec![AutomationPoint {
                tick: Tick(960),
                value: 200.0,
                curve: Curve::Linear,
            }],
        });
        let data = build_playback_data(&project, 48_000.0, &SampleBank::default());
        assert_eq!(data.tracks[0].device_auto.len(), 1);
        let (name, points) = &data.tracks[0].device_auto[0];
        assert_eq!(name, "cutoff");
        assert_eq!(points[0].sample, 24_000); // 120bpm: 960 tick = 0.5s
        assert!((points[0].value - 200.0).abs() < 1e-6);
    }

    #[test]
    fn cutoff_automation_sweeps_brightness() {
        use crate::export::render_project;
        use glaux_core::{AutomationLane, AutomationPoint, Curve, ParamPath};

        // 低カットオフ → 高カットオフのスイープ。後半ほど高域(隣接差分)が増える
        let mut project = project_with_notes(vec![note(0, 3840, 45, 110)]);
        let mut device = glaux_core::Device::builtin("subtractive");
        device.params.insert("sustain".into(), 1.0.into());
        device.params.insert("release".into(), 0.05.into());
        device.params.insert("filter_env".into(), 0.0.into()); // スイープはレーンだけで
        project.tracks[0].device = Some(device);
        project.tracks[0].automation.push(AutomationLane {
            target: ParamPath::device("cutoff"),
            points: vec![
                AutomationPoint {
                    tick: Tick(0),
                    value: 150.0,
                    curve: Curve::Hold, // 1 秒間こもったまま
                },
                AutomationPoint {
                    tick: Tick(1920), // 1 秒で全開に
                    value: 9000.0,
                    curve: Curve::Linear,
                },
            ],
        });
        let out = render_project(&project, 48_000.0, &Default::default()).unwrap();
        // 左 ch の「隣接差分 RMS / RMS」= 音量に依らない高域の割合で前半と後半を比較
        let brightness = |a: f64, b: f64| -> f32 {
            let s = (a * 48_000.0) as usize;
            let e = (b * 48_000.0) as usize;
            let (mut dd, mut ss) = (0.0f32, 0.0f32);
            for i in s..e {
                let d = out[(i + 1) * 2] - out[i * 2];
                dd += d * d;
                ss += out[i * 2] * out[i * 2];
            }
            (dd / ss.max(1e-12)).sqrt()
        };
        let head = brightness(0.2, 0.6);
        let tail = brightness(1.4, 1.9);
        assert!(
            tail > head * 2.0,
            "スイープで高域の割合が増えるはず: head={head} tail={tail}"
        );
    }

    #[test]
    fn effect_param_automation_changes_output() {
        use crate::export::render_project;
        use glaux_core::{AutomationLane, AutomationPoint, Curve, Effect, FxId, ParamPath};
        // 歪みの出力レベルを -24dB → +6dB に上げていく。後半ほど大きくなるはず
        let mut project = project_with_notes(vec![note(0, 3840, 57, 110)]);
        let fx_id = FxId::new();
        let mut dist = Effect::builtin(fx_id.clone(), "distortion");
        dist.params.insert("level_db".into(), (-24.0).into());
        project.tracks[0].effects.push(dist);
        project.tracks[0].automation.push(AutomationLane {
            target: ParamPath::effect(fx_id, "level_db"),
            points: vec![
                AutomationPoint {
                    tick: Tick(0),
                    value: -24.0,
                    curve: Curve::Linear,
                },
                AutomationPoint {
                    tick: Tick(3840),
                    value: 6.0,
                    curve: Curve::Linear,
                },
            ],
        });
        let data = build_playback_data(&project, 48_000.0, &SampleBank::default());
        assert_eq!(data.tracks[0].fx_auto.len(), 1);
        let out = render_project(&project, 48_000.0, &Default::default()).unwrap();
        let rms = |a: f64, b: f64| {
            let sl = &out[(a * 96_000.0) as usize..(b * 96_000.0) as usize];
            (sl.iter().map(|s| s * s).sum::<f32>() / sl.len() as f32).sqrt()
        };
        let head = rms(0.1, 0.4);
        let tail = rms(1.5, 1.9);
        assert!(
            tail > head * 5.0,
            "レベルが上がっていくはず: head={head} tail={tail}"
        );

        // バイパスしたエフェクトのレーンは無視される
        project.tracks[0].effects[0].bypass = true;
        let data = build_playback_data(&project, 48_000.0, &SampleBank::default());
        assert!(data.tracks[0].fx_auto.is_empty());
    }

    #[test]
    fn pan_automation_moves_left_to_right() {
        use crate::export::render_project;
        use glaux_core::{AutomationLane, AutomationPoint, Curve, ParamPath};

        let mut project = project_with_notes(vec![note(0, 3840, 69, 110)]);
        project.tracks[0].automation.push(AutomationLane {
            target: ParamPath::track("pan"),
            points: vec![
                AutomationPoint {
                    tick: Tick(0),
                    value: -1.0,
                    curve: Curve::Linear,
                },
                AutomationPoint {
                    tick: Tick(3840),
                    value: 1.0,
                    curve: Curve::Linear,
                },
            ],
        });
        let out = render_project(&project, 48_000.0, &Default::default()).unwrap();
        let n = out.len() / 2;
        let energy = |range: std::ops::Range<usize>, ch: usize| -> f32 {
            range.map(|i| out[i * 2 + ch].powi(2)).sum()
        };
        let head_l = energy(0..n / 4, 0);
        let head_r = energy(0..n / 4, 1);
        let tail_l = energy(n / 2..n * 3 / 4, 0);
        let tail_r = energy(n / 2..n * 3 / 4, 1);
        assert!(head_l > head_r * 5.0, "冒頭は左寄りのはず");
        assert!(tail_r > tail_l * 2.0, "後半は右寄りのはず");
    }

    #[test]
    fn tempo_segments_convert_both_ways() {
        use glaux_core::{TempoEvent, TempoMap};
        // 0〜3840 tick は 120bpm(25 samples/tick)、以降 60bpm(50 samples/tick)
        let mut project = project_with_notes(vec![]);
        project.tempo_map = TempoMap::new(vec![
            TempoEvent {
                tick: Tick(0),
                bpm: 120.0,
            },
            TempoEvent {
                tick: Tick(3840),
                bpm: 60.0,
            },
        ])
        .unwrap();
        let data = build_playback_data(&project, 48_000.0, &SampleBank::default());
        assert_eq!(data.tempo.len(), 2);
        assert_eq!(data.tempo[1].sample, 96_000);
        assert!((data.sample_to_tick(48_000) - 1920.0).abs() < 1e-9);
        assert!((data.sample_to_tick(96_000 + 50_000) - 4840.0).abs() < 1e-9);
        assert_eq!(data.tick_to_sample(1920.0), 48_000);
        assert_eq!(data.tick_to_sample(4840.0), 146_000);
    }

    #[test]
    fn next_beat_follows_time_signature() {
        use glaux_core::TimeSigEvent;
        let mut project = project_with_notes(vec![]);
        project.time_sig_map = vec![
            TimeSigEvent {
                tick: Tick(0),
                num: 4,
                den: 4,
            },
            TimeSigEvent {
                tick: Tick(3840),
                num: 7,
                den: 8,
            },
        ];
        let data = build_playback_data(&project, 48_000.0, &SampleBank::default());
        // 120bpm: 1 拍(4 分)= 24000 サンプル
        let b0 = data.next_beat(0).unwrap();
        assert_eq!((b0.sample, b0.downbeat), (0, true));
        let b1 = data.next_beat(1).unwrap();
        assert_eq!((b1.sample, b1.downbeat), (24_000, false));
        // 2 小節目(tick 3840 = 96000)からは 8 分拍 = 12000 サンプル、7 拍で小節
        let b = data.next_beat(96_001).unwrap();
        assert_eq!((b.sample, b.downbeat), (108_000, false));
        let bar3 = data.next_beat(96_000 + 12_000 * 7).unwrap();
        assert!(bar3.downbeat);
    }

    #[test]
    fn tempo_change_keeps_musical_position() {
        use crate::render::{Renderer, Shared};
        use glaux_core::{TempoEvent, TempoMap};
        use std::sync::atomic::Ordering;
        use std::sync::Arc;

        let mk = |bpm: f64| {
            let mut p = project_with_notes(vec![note(0, 3840 * 4, 60, 100)]);
            p.tempo_map = TempoMap::new(vec![TempoEvent { tick: Tick(0), bpm }]).unwrap();
            build_playback_data(&p, 48_000.0, &SampleBank::default())
        };
        let shared = Arc::new(Shared::new(mk(120.0)));
        shared.playing.store(true, Ordering::Release);
        let mut r = Renderer::new(shared.clone());
        let mut buf = vec![0.0f32; 4800 * 2];
        for _ in 0..10 {
            r.process(&mut buf, 2); // 1 秒 = tick 1920 @120bpm
        }
        assert_eq!(shared.pos.load(Ordering::Acquire), 48_000);

        // 60bpm に差し替え: tick 1920 は 2 秒 = 96000 サンプルに相当する
        shared.data.store(Arc::new(mk(60.0)));
        r.process(&mut buf, 2);
        assert_eq!(
            shared.pos.load(Ordering::Acquire),
            96_000 + 4800,
            "テンポ変更後も音楽的位置(tick)が保たれるはず"
        );
    }

    /// 1 秒のサイン波 WAV を書き、それを音声クリップとして置いたプロジェクトを作る
    fn audio_clip_project(dir: &std::path::Path, clip_start: u64, clip_len: u64) -> Project {
        use glaux_core::{Asset, AssetId, TrackKind};
        std::fs::create_dir_all(dir.join("audio")).unwrap();
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: 48_000,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut w = hound::WavWriter::create(dir.join("audio/tone.wav"), spec).unwrap();
        for i in 0..48_000 {
            let s = (i as f32 * 440.0 * std::f32::consts::TAU / 48_000.0).sin();
            w.write_sample((s * 20_000.0) as i16).unwrap();
        }
        w.finalize().unwrap();

        let mut project = Project::new("a");
        let asset_id = AssetId::from_sha256_hex("abcd").unwrap();
        project.assets.insert(
            asset_id.clone(),
            Asset {
                path: "audio/tone.wav".into(),
                sample_rate: 48_000,
                channels: 1,
                frames: 48_000,
            },
        );
        let mut track = Track::new(TrackId::new(), "Gt", TrackKind::Audio);
        track.clips.push(Clip::new_audio(
            ClipId::new(),
            "take",
            Tick(clip_start),
            Tick(clip_len),
            asset_id,
        ));
        project.tracks.push(track);
        project
    }

    #[test]
    fn audio_clip_plays_in_its_region_only() {
        use crate::export::render_project;
        let tmp = tempfile::tempdir().unwrap();
        // クリップは 0.5 秒(tick 960)から 1 小節(2 秒)だが波形は 1 秒しかない
        let project = audio_clip_project(tmp.path(), 960, 3840);
        let bank = SampleBank::load(&project, tmp.path());
        let data = build_playback_data(&project, 48_000.0, &bank);
        assert_eq!(data.audio_events.len(), 1);
        assert_eq!(data.audio_events[0].start, 24_000);
        assert_eq!(data.end_sample, 24_000 + 96_000);

        let out = render_project(&project, 48_000.0, &bank).unwrap();
        let rms = |a: f64, b: f64| {
            let e = ((b * 96_000.0) as usize).min(out.len());
            let sl = &out[(a * 96_000.0) as usize..e];
            (sl.iter().map(|s| s * s).sum::<f32>() / sl.len() as f32).sqrt()
        };
        assert!(rms(0.0, 0.4) < 1e-4, "クリップ前は無音");
        assert!(rms(0.6, 1.4) > 0.2, "クリップ区間は鳴る");
        assert!(rms(1.6, 1.9) < 1e-4, "波形を読み切ったら止まる");
    }

    #[test]
    fn audio_clip_resumes_mid_clip_after_seek_and_pause() {
        use crate::render::{Renderer, Shared};
        use std::sync::atomic::Ordering;
        use std::sync::Arc;
        let tmp = tempfile::tempdir().unwrap();
        let project = audio_clip_project(tmp.path(), 0, 3840);
        let bank = SampleBank::load(&project, tmp.path());
        let shared = Arc::new(Shared::new(build_playback_data(&project, 48_000.0, &bank)));
        let mut r = Renderer::new(shared.clone());
        let mut buf = vec![0.0f32; 4800 * 2];
        let rms = |b: &[f32]| (b.iter().map(|s| s * s).sum::<f32>() / b.len() as f32).sqrt();

        // クリップ途中(0.5 秒)へシークして再生 → 鳴る
        shared.seek.store(24_000, Ordering::Release);
        shared.playing.store(true, Ordering::Release);
        r.process(&mut buf, 2);
        assert!(rms(&buf) > 0.2, "途中からでも鳴るはず");

        // 一時停止 → 無音、再開 → 続きから鳴る
        shared.playing.store(false, Ordering::Release);
        r.process(&mut buf, 2);
        assert!(rms(&buf) < 1e-6);
        shared.playing.store(true, Ordering::Release);
        r.process(&mut buf, 2);
        assert!(rms(&buf) > 0.2, "再開後も鳴るはず");
    }

    #[test]
    fn pan_and_volume_shape_gains() {
        let mut project = project_with_notes(vec![]);
        project.tracks[0].pan = -1.0; // full L
        project.tracks[0].volume_db = -6.0;
        let data = build_playback_data(&project, 48_000.0, &SampleBank::default());
        let t = &data.tracks[0];
        assert!(t.gain_l > 0.0);
        assert!(t.gain_r.abs() < 1e-6);
        // -6dB ≒ 0.5012
        assert!((t.gain_l - db_to_amp(-6.0)).abs() < 1e-6);
    }
}
