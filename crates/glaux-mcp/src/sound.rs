//! 音色の解析・比較の対象になる「音」を用意する(UI と MCP で共用)。
//!
//! 対象は 3 種類:
//! - 音声クリップ(クリップが参照している範囲。テンポ追従中も素材の範囲を正しく切り出す)
//! - 音声ファイル(WAV / MP3 / FLAC / OGG / M4A)
//! - トラックの音源で鳴らした 1 音(`render_track_note`。CLAP プラグインも鳴る)

use glaux_core::{ClipContent, ClipId, Project, TrackId};
use std::path::{Path, PathBuf};

/// 解析・比較の対象。
#[derive(Clone, Debug)]
pub enum SoundSource {
    Clip(ClipId),
    File(PathBuf),
    TrackNote {
        track: TrackId,
        pitch: u8,
        velocity: u8,
        seconds: f64,
    },
}

/// 読み込んだ音(モノラル)。
pub struct LoadedSound {
    pub frames: Vec<f32>,
    pub sample_rate: f32,
    /// 何の音か(表示用)
    pub label: String,
}

/// 試し鳴らしのサンプルレート
pub const RENDER_RATE: f64 = 48_000.0;

/// 音声クリップが参照している範囲をモノラルで読む。
pub fn load_clip(project: &Project, dir: &Path, clip_id: &ClipId) -> Result<LoadedSound, String> {
    let clip = project
        .tracks
        .iter()
        .find_map(|t| t.clips.iter().find(|c| &c.id == clip_id))
        .ok_or_else(|| format!("クリップが見つかりません: {clip_id}"))?;
    let ClipContent::Audio {
        asset,
        offset_samples,
        stretch,
        ..
    } = &clip.content
    else {
        return Err(format!("{clip_id} は音声クリップではありません"));
    };
    let meta = project
        .assets
        .get(asset)
        .ok_or_else(|| format!("アセットが見つかりません: {asset}"))?;
    let data = glaux_engine::load_wav_mono(&dir.join(&meta.path))?;
    let secs = stretch
        .follow_seconds(clip.length.0 as f64)
        .unwrap_or_else(|| {
            project.tempo_map.tick_to_seconds(clip.start + clip.length)
                - project.tempo_map.tick_to_seconds(clip.start)
        });
    let from = (*offset_samples as usize).min(data.frames.len());
    let to = (from + (secs * data.sample_rate as f64) as usize).min(data.frames.len());
    Ok(LoadedSound {
        frames: data.frames[from..to].to_vec(),
        sample_rate: data.sample_rate,
        label: format!("音声クリップ「{}」", clip.name),
    })
}

/// 音声ファイルをモノラルで読む。
pub fn load_file(path: &Path) -> Result<LoadedSound, String> {
    let ext = path
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    let label = format!(
        "ファイル「{}」",
        path.file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default()
    );
    if ext == "wav" {
        let d = glaux_engine::load_wav_mono(path)?;
        return Ok(LoadedSound {
            frames: d.frames,
            sample_rate: d.sample_rate,
            label,
        });
    }
    let bytes =
        std::fs::read(path).map_err(|e| format!("読み込めません({}): {e}", path.display()))?;
    let (inter, ch, sr) = crate::assets::decode_audio(bytes, &ext)?;
    let ch = ch.max(1) as usize;
    let frames = inter
        .chunks(ch)
        .map(|c| c.iter().sum::<f32>() / c.len() as f32)
        .collect();
    Ok(LoadedSound {
        frames,
        sample_rate: sr as f32,
        label,
    })
}

/// トラックの音源で 1 音鳴らす。
pub fn render_note(
    project: &Project,
    dir: &Path,
    track: &TrackId,
    pitch: u8,
    velocity: u8,
    seconds: f64,
) -> Result<LoadedSound, String> {
    let t = project
        .track(track)
        .ok_or_else(|| format!("トラックが見つかりません: {track}"))?;
    let bank = glaux_engine::SampleBank::for_offline(project, dir);
    let frames = glaux_engine::render_track_note(
        project,
        track,
        pitch,
        velocity,
        seconds,
        RENDER_RATE,
        &bank,
    )
    .map_err(|e| format!("「{}」の音を鳴らせません(MIDI トラックのみ): {e}", t.name))?;
    Ok(LoadedSound {
        frames,
        sample_rate: RENDER_RATE as f32,
        label: format!(
            "トラック「{}」の音(MIDI {pitch}、ベロシティ {velocity})",
            t.name
        ),
    })
}

pub fn load(project: &Project, dir: &Path, source: &SoundSource) -> Result<LoadedSound, String> {
    let s = match source {
        SoundSource::Clip(id) => load_clip(project, dir, id)?,
        SoundSource::File(p) => load_file(p)?,
        SoundSource::TrackNote {
            track,
            pitch,
            velocity,
            seconds,
        } => render_note(project, dir, track, *pitch, *velocity, *seconds)?,
    };
    if s.frames.iter().all(|v| v.abs() < 1e-6) {
        return Err(format!("{} が無音です", s.label));
    }
    Ok(s)
}

/// 音程を推定する。SwiftF0(学習済みモデル)を使い、失敗したら `None`(呼び出し側で内蔵の YIN に任せる)。
pub fn pitch_track(sound: &LoadedSound) -> Option<Vec<glaux_engine::timbre::PitchFrame>> {
    use glaux_engine::timbre::{resample_pitch, PitchFrame};
    let est = glaux_ml::pitch::track(&sound.frames, sound.sample_rate).ok()?;
    let frames: Vec<PitchFrame> = est
        .iter()
        .map(|e| PitchFrame {
            time: e.time,
            f0: if e.confidence >= 0.5 { e.f0 } else { 0.0 },
            confidence: e.confidence,
        })
        .collect();
    Some(resample_pitch(
        &frames,
        sound.frames.len() as f32 / sound.sample_rate,
    ))
}

/// 音色記述子を求める(音程は SwiftF0。使えなければ内蔵の YIN)。
pub fn describe(sound: &LoadedSound) -> glaux_engine::timbre::SoundDescriptors {
    let pitch = pitch_track(sound);
    glaux_engine::timbre::describe(&sound.frames, sound.sample_rate, pitch.as_deref())
}

/// 音声のビート・小節頭・テンポ(Beat This!)。
#[derive(Clone, Debug, serde::Serialize)]
pub struct BeatReport {
    pub source: String,
    pub duration_sec: f64,
    /// 平均テンポ(BPM、小数 2 桁)。ビートが少なすぎれば null
    pub bpm: Option<f64>,
    /// 半分・倍のテンポ(ビートの取り方の解釈違いの候補)
    pub bpm_alternatives: Vec<f64>,
    /// テンポの揺れ(拍の長さに対する割合)。0.015 未満は打ち込み・クリックに合わせた演奏
    pub tempo_variation: Option<f64>,
    /// 1 小節のビート数(拍子の分子の推定)
    pub beats_per_bar: Option<u32>,
    /// 最初の小節頭の時刻(秒。素材の先頭から)
    pub first_downbeat_sec: Option<f64>,
    pub beat_count: usize,
    pub downbeat_count: usize,
    /// ビートの時刻(秒。先頭から最大 256 個)
    pub beats: Vec<f64>,
    /// 小節頭の時刻(秒。先頭から最大 128 個)
    pub downbeats: Vec<f64>,
    /// 言葉での要約
    pub summary: String,
}

/// ビート・小節頭・テンポを推定する。
pub fn beats(sound: &LoadedSound) -> Result<BeatReport, String> {
    let t = glaux_ml::beats::track(&sound.frames, sound.sample_rate).map_err(|e| e.to_string())?;
    // JSON に f32 の誤差(0.0199999…)が出ないよう f64 で丸める
    let r2 = |v: f32| (v as f64 * 100.0).round() / 100.0;
    let r3 = |v: f32| (v as f64 * 1000.0).round() / 1000.0;
    let bpm = t.bpm().map(r2);
    let variation = t.tempo_variation().map(r3);
    let bpb = t.beats_per_bar();
    let summary = match bpm {
        None => "ビートが見つかりません(拍のはっきりしない音か、短すぎます)".to_owned(),
        Some(b) => {
            let feel = match variation {
                Some(v) if v < 0.015 => "テンポは一定(打ち込み・クリックに合わせた演奏)",
                Some(v) if v < 0.04 => "テンポに人の演奏らしい揺れがある",
                Some(_) => "テンポが大きく揺れる(ルバート・テンポの変化があるか、拍が取りにくい)",
                None => "",
            };
            let meter = bpb.map(|n| format!("、1 小節 {n} 拍")).unwrap_or_default();
            format!("約 {b} BPM{meter}。{feel}")
        }
    };
    Ok(BeatReport {
        source: sound.label.clone(),
        duration_sec: r2(sound.frames.len() as f32 / sound.sample_rate),
        bpm,
        bpm_alternatives: bpm
            .map(|b| {
                [b / 2.0, b * 2.0]
                    .into_iter()
                    .filter(|v| (40.0..=240.0).contains(v))
                    .map(|v| (v * 100.0).round() / 100.0)
                    .collect()
            })
            .unwrap_or_default(),
        tempo_variation: variation,
        beats_per_bar: bpb,
        first_downbeat_sec: t.downbeats.first().copied().map(r3),
        beat_count: t.beats.len(),
        downbeat_count: t.downbeats.len(),
        beats: t.beats.iter().take(256).copied().map(r3).collect(),
        downbeats: t.downbeats.iter().take(128).copied().map(r3).collect(),
        summary,
    })
}

/// 音色語のカテゴリ(表示順)。
pub const WORD_CATEGORIES: [&str; 7] = [
    "instrument",
    "tone",
    "texture",
    "envelope",
    "movement",
    "space",
    "mood",
];

/// CLAP の埋め込み(モデルが無ければエラー)。
pub fn embedding(sound: &LoadedSound) -> Result<Vec<f32>, String> {
    glaux_ml::clap::embed(&sound.frames, sound.sample_rate).map_err(|e| e.to_string())
}

/// 埋め込みを音色語で表す: カテゴリごとに近い 3 語({ja, en, z})。
pub fn words_json(embedding: &[f32]) -> serde_json::Value {
    let words = glaux_ml::clap::describe(embedding, 3);
    let mut obj = serde_json::Map::new();
    for c in WORD_CATEGORIES {
        let list: Vec<serde_json::Value> = words
            .iter()
            .filter(|w| w.category == c)
            .map(|w| {
                serde_json::json!({
                    "ja": w.ja,
                    "en": w.en,
                    "z": (w.z as f64 * 10.0).round() / 10.0,
                })
            })
            .collect();
        obj.insert(c.to_owned(), serde_json::Value::Array(list));
    }
    serde_json::Value::Object(obj)
}

/// CLAP のモデルが無いときに AI・人間に伝える文。
pub fn clap_missing_note() -> String {
    format!(
        "音色を言葉で捉えるモデル(CLAP、約 {} MB)が未取得です。設定の「追加モデル」から取得できます",
        glaux_ml::clap::MODEL_BYTES / 1_000_000
    )
}

// ---- 比べる・似せる ----

/// 距離を言葉にする。
pub fn verdict(total: f32) -> &'static str {
    if total < 0.15 {
        "ほぼ同じ音"
    } else if total < 0.35 {
        "よく似ている"
    } else if total < 0.7 {
        "似ている部分がある"
    } else {
        "かなり違う"
    }
}

/// 2 音の違いを観点ごとに言葉にする(A を基準に B がどうか)。
fn difference_hints(
    a: &glaux_engine::timbre::SoundDescriptors,
    b: &glaux_engine::timbre::SoundDescriptors,
) -> Vec<String> {
    let mut out = Vec::new();
    let ratio = |x: f32, y: f32| y.max(1e-6) / x.max(1e-6);
    let r = ratio(a.spectrum.centroid_hz, b.spectrum.centroid_hz);
    if r > 1.25 {
        out.push(format!(
            "B の方が明るい(明るさの中心 {:.0} → {:.0} Hz)。B を A に寄せるならカットオフを下げる・高域を削る",
            a.spectrum.centroid_hz, b.spectrum.centroid_hz
        ));
    } else if r < 0.8 {
        out.push(format!(
            "B の方が暗い(明るさの中心 {:.0} → {:.0} Hz)。B を A に寄せるならカットオフを上げる・高域を足す",
            a.spectrum.centroid_hz, b.spectrum.centroid_hz
        ));
    }
    let (ea, eb) = (&a.envelope, &b.envelope);
    let att = eb.attack_ms - ea.attack_ms;
    if att.abs() > 10.0_f32.max(ea.attack_ms * 0.5) {
        out.push(format!(
            "B の立ち上がりが{}(アタック {:.0} → {:.0} ms)",
            if att > 0.0 { "遅い" } else { "速い" },
            ea.attack_ms,
            eb.attack_ms
        ));
    }
    if ea.decays_continuously != eb.decays_continuously {
        out.push(if eb.decays_continuously {
            "B は減衰し続ける(A は伸びる)。B を A に寄せるならサスティンを上げる".to_owned()
        } else {
            "B は伸びる(A は減衰し続ける)。B を A に寄せるならサスティンを下げてディケイで減衰させる".to_owned()
        });
    } else if (eb.sustain_db - ea.sustain_db).abs() > 6.0 && !ea.decays_continuously {
        out.push(format!(
            "持続部の音量が違う(サスティン {:.0} → {:.0} dB)",
            ea.sustain_db, eb.sustain_db
        ));
    }
    let len = eb.active_ms - ea.active_ms;
    if len.abs() > 150.0_f32.max(ea.active_ms * 0.3) {
        out.push(format!(
            "B の方が{}(鳴っている長さ {:.0} → {:.0} ms)",
            if len > 0.0 { "長い" } else { "短い" },
            ea.active_ms,
            eb.active_ms
        ));
    }
    let fl = b.spectrum.flatness - a.spectrum.flatness;
    if fl.abs() > 0.1 {
        out.push(format!(
            "B の方がノイズっぽさが{}(平坦さ {:.2} → {:.2})",
            if fl > 0.0 { "強い" } else { "弱い" },
            a.spectrum.flatness,
            b.spectrum.flatness
        ));
    }
    if let (Some(ha), Some(hb)) = (&a.harmonics, &b.harmonics) {
        if ha.waveform_guess != hb.waveform_guess {
            out.push(format!(
                "倍音の並びが違う(波形の推定 {} → {})",
                ha.waveform_guess, hb.waveform_guess
            ));
        }
        if (hb.odd_even_db - ha.odd_even_db).abs() > 6.0 {
            out.push(format!(
                "奇数倍音の多さが違う({:.0} → {:.0} dB。大きいほど矩形波・クラリネット寄り)",
                ha.odd_even_db, hb.odd_even_db
            ));
        }
    }
    match (&a.pitch, &b.pitch) {
        (Some(pa), Some(pb)) => {
            let cents = 1200.0 * (pb.f0_hz / pa.f0_hz).log2();
            if cents.abs() > 30.0 {
                out.push(format!(
                    "音の高さが違う({:.0} → {:.0} Hz、{:+.0} セント)",
                    pa.f0_hz, pb.f0_hz, cents
                ));
            }
            if (pb.vibrato_depth_cents - pa.vibrato_depth_cents).abs() > 15.0 {
                out.push(format!(
                    "ビブラートの深さが違う(± {:.0} → {:.0} セント)",
                    pa.vibrato_depth_cents, pb.vibrato_depth_cents
                ));
            }
        }
        (Some(_), None) => {
            out.push("A には音程があるが、B は音程が取れない(打楽器・ノイズ的)".to_owned())
        }
        (None, Some(_)) => {
            out.push("B には音程があるが、A は音程が取れない(打楽器・ノイズ的)".to_owned())
        }
        _ => {}
    }
    out
}

/// 2 音を比べる: 距離(音色・音量の時間変化)、観点ごとの違い、CLAP での近さ。
pub fn compare(a: &LoadedSound, b: &LoadedSound) -> Result<serde_json::Value, String> {
    use glaux_engine::sound_match;
    let d = sound_match::compare(&a.frames, a.sample_rate, &b.frames, b.sample_rate);
    let (da, db) = (describe(a), describe(b));
    let r3 = |v: f32| (v as f64 * 1000.0).round() / 1000.0;
    let mut v = serde_json::json!({
        "a": a.label,
        "b": b.label,
        "distance": {
            "total": r3(d.total),
            "spectral": r3(d.spectral),
            "envelope": r3(d.envelope),
        },
        "verdict": verdict(d.total),
        "differences": difference_hints(&da, &db),
    });
    if glaux_ml::clap::available() {
        let (ea, eb) = (embedding(a)?, embedding(b)?);
        v["clap_similarity"] = serde_json::json!(r3(glaux_ml::clap::similarity(&ea, &eb)));
    }
    Ok(v)
}

/// 目標の音に内蔵 subtractive を合わせた結果。
pub struct MatchOutcome {
    pub fit: glaux_engine::sound_match::FitResult,
    /// 目標の音の高さ(MIDI)と、鍵盤を押しておく秒数
    pub pitch: u8,
    pub hold: f32,
    pub descriptors: glaux_engine::timbre::SoundDescriptors,
}

/// 目標の音の高さ(MIDI)。音程が取れない非調和な音(ベル等)はスペクトルの主要な成分から、それも無ければ 60。
pub fn target_pitch(sound: &LoadedSound, d: &glaux_engine::timbre::SoundDescriptors) -> u8 {
    d.pitch
        .as_ref()
        .map(|p| p.midi)
        .or_else(|| glaux_engine::timbre::dominant_pitch(&sound.frames, sound.sample_rate))
        .unwrap_or(60)
}

/// 鍵盤を押していた秒数の推定: 鳴っている長さ − 余韻(最後の減衰)。
/// 減衰し続ける音でも、鍵盤を離してから減り方が変わる所を余韻の始まりとみなせる。
pub fn estimate_hold(d: &glaux_engine::timbre::SoundDescriptors) -> f32 {
    let e = &d.envelope;
    ((e.active_ms - e.release_ms) / 1000.0).clamp(0.05, 3.0)
}

/// 目標の音に内蔵 subtractive のつまみを合わせる(CMA-ES)。
pub fn match_subtractive(target: &LoadedSound, max_seconds: f32) -> MatchOutcome {
    match_sound(
        target,
        Some(glaux_engine::sound_match::FitInstrument::Subtractive),
        false,
        max_seconds,
    )
}

/// 目標の音に内蔵音源のつまみを合わせる(CMA-ES)。`instrument` が None なら subtractive / fm / wavetable を
/// 時間を等分して探して最も近いもの、`reverb` ならリバーブ(mix / size)も一緒に探す。
pub fn match_sound(
    target: &LoadedSound,
    instrument: Option<glaux_engine::sound_match::FitInstrument>,
    reverb: bool,
    max_seconds: f32,
) -> MatchOutcome {
    use glaux_engine::sound_match::{fit_instrument, FitInstrument, FitOptions};
    let d = describe(target);
    let pitch = target_pitch(target, &d);
    let hold = estimate_hold(&d);
    let secs = max_seconds.clamp(2.0, 120.0);
    let candidates: Vec<FitInstrument> = match instrument {
        Some(i) => vec![i],
        None => vec![
            FitInstrument::Subtractive,
            FitInstrument::Fm,
            FitInstrument::Wavetable,
        ],
    };
    let opts = FitOptions {
        max_seconds: secs / candidates.len() as f32,
        reverb,
        ..Default::default()
    };
    let mut results: Vec<glaux_engine::sound_match::FitResult> = candidates
        .iter()
        .map(|i| {
            fit_instrument(
                &target.frames,
                target.sample_rate,
                pitch,
                hold,
                &d,
                *i,
                opts,
            )
        })
        .collect();
    results.sort_by(|a, b| a.distance.total.total_cmp(&b.distance.total));
    let mut fit = results.remove(0);
    // 自動選択のとき: 比べた音源ごとの距離も残す
    for other in &results {
        fit.tried.push((
            format!("{}(不採用)", other.instrument.name()),
            other.distance.total,
        ));
        fit.evaluations += other.evaluations;
    }
    MatchOutcome {
        fit,
        pitch,
        hold,
        descriptors: d,
    }
}

/// 合わせた結果の内蔵音源の Device(同じ音源の今の gain_db があれば保つ)。
pub fn matched_device(
    outcome: &MatchOutcome,
    current: Option<&glaux_core::Device>,
) -> glaux_core::Device {
    let name = outcome.fit.instrument.name();
    let mut device = glaux_core::Device::builtin(name);
    device.params = outcome.fit.params.clone();
    let keep_gain = current.and_then(|d| match &d.source {
        glaux_core::PluginSource::Builtin { name: n } if n == name => {
            d.params.get("gain_db").cloned()
        }
        _ => None,
    });
    if let Some(g) = keep_gain {
        device.params.insert("gain_db".into(), g);
    }
    device
}

/// 合わせたリバーブを挿すエフェクト(リバーブも探して、効きがあるときだけ)。
pub fn matched_reverb(outcome: &MatchOutcome) -> Option<glaux_core::Effect> {
    let (mix, size) = outcome.fit.reverb?;
    if mix < 0.03 {
        return None;
    }
    let mut e = glaux_core::Effect::builtin(glaux_core::FxId::new(), "reverb");
    e.params.insert(
        "mix".into(),
        glaux_core::ParamValue::Float((mix as f64 * 1000.0).round() / 1000.0),
    );
    e.params.insert(
        "size".into(),
        glaux_core::ParamValue::Float((size as f64 * 1000.0).round() / 1000.0),
    );
    Some(e)
}

/// 合わせた結果を AI・UI 向けの JSON にする。
pub fn match_json(o: &MatchOutcome) -> serde_json::Value {
    let r3 = |v: f32| (v as f64 * 1000.0).round() / 1000.0;
    let params: serde_json::Map<String, serde_json::Value> = o
        .fit
        .params
        .iter()
        .map(|(k, v)| {
            let j = match v {
                glaux_core::ParamValue::Float(f) => {
                    serde_json::json!((f * 1000.0).round() / 1000.0)
                }
                other => serde_json::to_value(other).unwrap_or_default(),
            };
            (k.clone(), j)
        })
        .collect();
    serde_json::json!({
        "instrument": o.fit.instrument.name(),
        "params": params,
        "reverb": o.fit.reverb.map(|(mix, size)| serde_json::json!({ "mix": r3(mix), "size": r3(size) })),
        "pitch": o.pitch,
        "hold_seconds": r3(o.hold),
        "distance": r3(o.fit.distance.total),
        "distance_detail": {
            "spectral": r3(o.fit.distance.spectral),
            "envelope": r3(o.fit.distance.envelope),
        },
        "initial_distance": r3(o.fit.initial_distance.total),
        "verdict": verdict(o.fit.distance.total),
        "variants_tried": o.fit.tried.iter().map(|(w, d)| serde_json::json!({"variant": w, "distance": r3(*d)})).collect::<Vec<_>>(),
        "evaluations": o.fit.evaluations,
        "seconds": (o.fit.seconds * 10.0).round() / 10.0,
    })
}

/// 音声クリップに似せた内蔵シンセのトラックを作るコマンド(クリップのトラックの直後に置き、
/// クリップと同じ位置に目標の高さ・長さの 1 音を置く)。
pub struct MatchClip {
    pub commands: Vec<glaux_core::Command>,
    pub track_id: TrackId,
    pub track_name: String,
    pub outcome: MatchOutcome,
}

pub fn match_clip_commands(
    project: &Project,
    dir: &Path,
    clip_id: &ClipId,
    max_seconds: f32,
) -> Result<MatchClip, String> {
    use glaux_core::{Clip, Command, Note, NoteId, Tick, Track, TrackKind};
    let (index, clip) = project
        .tracks
        .iter()
        .enumerate()
        .find_map(|(i, t)| t.clips.iter().find(|c| &c.id == clip_id).map(|c| (i, c)))
        .ok_or_else(|| format!("クリップが見つかりません: {clip_id}"))?;
    let target = load(project, dir, &SoundSource::Clip(clip_id.clone()))?;
    let outcome = match_sound(&target, None, true, max_seconds);
    let track_id = TrackId::new();
    let track_name = format!("{} の再現", clip.name);
    let mut track = Track::new(track_id.clone(), track_name.clone(), TrackKind::Midi);
    track.device = Some(matched_device(&outcome, None));
    if let Some(rv) = matched_reverb(&outcome) {
        track.effects.push(rv);
    }
    let tm = &project.tempo_map;
    let start_sec = tm.tick_to_seconds(clip.start);
    let end = tm.seconds_to_tick(start_sec + outcome.hold as f64);
    let dur = Tick(end.0.saturating_sub(clip.start.0).max(1));
    let mut midi = Clip::new_midi(
        glaux_core::ClipId::new(),
        track_name.clone(),
        clip.start,
        Tick(dur.0 + glaux_core::time::PPQ),
    );
    if let ClipContent::Midi { notes, .. } = &mut midi.content {
        notes.push(Note {
            id: NoteId::new(),
            pos: Tick::ZERO,
            dur,
            pitch: outcome.pitch,
            vel: 100,
            articulation: Default::default(),
            pitch_curve: vec![],
            glide_ms: None,
        });
    }
    let commands = vec![
        Command::AddTrack {
            track,
            index: Some(index + 1),
        },
        Command::AddClip {
            track: track_id.clone(),
            clip: midi,
        },
    ];
    Ok(MatchClip {
        commands,
        track_id,
        track_name,
        outcome,
    })
}
