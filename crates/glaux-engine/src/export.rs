//! オフラインレンダリングと WAV 書き出し。
//!
//! リアルタイム再生と同じ [`Renderer`](crate::render::Renderer) を使うので、
//! 「聴こえている音がそのまま書き出される」ことが保証される。
//! オーディオデバイスは不要(解析やヘッドレス環境でも使える)。

use crate::data::build_playback_data;
use crate::render::{Renderer, Shared};
use glaux_core::Project;
use std::path::Path;
use std::sync::atomic::Ordering;
use std::sync::Arc;

#[derive(Debug, thiserror::Error)]
pub enum ExportError {
    #[error("プロジェクトにノートがありません")]
    Empty,
    #[error("WAV の書き込みに失敗: {0}")]
    Wav(#[from] hound::Error),
    #[error("書き込み先を作成できません: {0}")]
    Io(#[from] std::io::Error),
    #[error("FLAC の書き出しに失敗: {0}")]
    Flac(String),
}

/// 書き出すファイルの形式。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AudioFormat {
    /// 16 / 24bit 整数、32bit 浮動小数
    #[default]
    Wav,
    /// 可逆圧縮(WAV の半分前後の大きさ)。16 / 24bit 整数のみ
    Flac,
}

impl AudioFormat {
    pub fn extension(self) -> &'static str {
        match self {
            AudioFormat::Wav => "wav",
            AudioFormat::Flac => "flac",
        }
    }
}

/// プロジェクト全体をステレオ・インターリーブの f32 にレンダリングする。
/// 終端はレンダラの自動停止(余韻込み)に任せ、末尾の無音は切り詰める。
pub fn render_project(
    project: &Project,
    sample_rate: f64,
    bank: &crate::data::SampleBank,
) -> Result<Vec<f32>, ExportError> {
    render_inner(project, sample_rate, bank, None, true)
}

/// マスターのクリップ防止を通さずに描き出す(トラックを音声にする用。0dBFS を超える音もそのまま)。
/// 呼び出し側で、対象のトラックだけを残してマスターのエフェクトを外したプロジェクトを渡す
pub fn render_stem(
    project: &Project,
    sample_rate: f64,
    bank: &crate::data::SampleBank,
) -> Result<Vec<f32>, ExportError> {
    render_inner(project, sample_rate, bank, None, false)
}

/// 範囲の手前から描き出す長さ(残響・リリース・コンプの立ち上がりの分)
const PREROLL_SECS: f64 = 3.0;
/// 範囲の頭で鳴っている長い音のために遡る上限
const MAX_PREROLL_SECS: f64 = 30.0;

/// 秒の範囲 `[from, to)` だけを描き出す(解析用。曲全体を描き出すより速い)。
/// 範囲の少し手前から描き出して、残響やリリースを含めた「範囲の頭で聞こえている音」にする。
/// 範囲の頭で鳴っているノート・音声クリップは、その頭から鳴らす(最大 30 秒遡る)。
/// 返すのは範囲の分だけ(ステレオ・インターリーブ)。末尾の無音は切り詰めない
pub fn render_project_range(
    project: &Project,
    sample_rate: f64,
    bank: &crate::data::SampleBank,
    from_secs: f64,
    to_secs: f64,
) -> Result<Vec<f32>, ExportError> {
    let from = (from_secs.max(0.0) * sample_rate) as u64;
    let to = ((to_secs * sample_rate) as u64).max(from);
    render_inner(project, sample_rate, bank, Some((from, to)), true)
}

fn render_inner(
    project: &Project,
    sample_rate: f64,
    bank: &crate::data::SampleBank,
    range: Option<(u64, u64)>,
    master_clip: bool,
) -> Result<Vec<f32>, ExportError> {
    let slots = Arc::new(crate::plugins::new_slots());
    // CLAP の音源・エフェクトがあれば、このスレッドで書き出し専用のインスタンスを作る
    let has_plugins = !crate::plugins::project_plugins(project).is_empty();
    let (offline, data) = if has_plugins {
        let (offline, map) = crate::plugins::OfflinePlugins::create(project, sample_rate, &slots);
        let mut bank = bank.clone();
        bank.plugin_slots = map;
        (
            Some(offline),
            build_playback_data(project, sample_rate, &bank),
        )
    } else {
        (None, build_playback_data(project, sample_rate, bank))
    };
    if data.events.is_empty() && data.audio_events.is_empty() {
        if let Some(o) = offline {
            o.finish(slots.iter().filter_map(|s| s.take_incoming()).collect());
        }
        return Err(ExportError::Empty);
    }
    // 範囲指定なら、描き出しの開始位置(範囲の頭で鳴っている音の頭まで遡る)
    let start = range.map(|(from, _)| {
        let preroll = (PREROLL_SECS * sample_rate) as u64;
        let limit = from.saturating_sub((MAX_PREROLL_SECS * sample_rate) as u64);
        let sounding = data
            .events
            .iter()
            .filter(|e| e.start < from && e.end > from)
            .map(|e| e.start)
            .chain(
                data.audio_events
                    .iter()
                    .filter(|a| a.start < from && a.end > from)
                    .map(|a| a.start),
            )
            .min()
            .unwrap_or(from);
        sounding.min(from.saturating_sub(preroll)).max(limit)
    });
    let mut shared = Shared::new(data);
    shared.plugin_slots = slots;
    let shared = Arc::new(shared);
    shared.playing.store(true, Ordering::Release);
    shared.no_master_clip.store(!master_clip, Ordering::Release);
    if let Some(start) = start {
        shared.seek.store(start, Ordering::Release);
    }
    let mut renderer = Renderer::new(shared.clone());

    const BLOCK: usize = 4096;
    // 安全上限: 曲の終端 + 10 秒(自動停止が先に来るのが通常)。範囲指定なら範囲の終わりまで
    let cap = match (range, start) {
        (Some((_, to)), Some(start)) => (to - start) as usize,
        _ => {
            let d = shared.data.load();
            (d.end_sample + (10.0 * sample_rate) as u64) as usize
        }
    };

    let mut out: Vec<f32> = Vec::new();
    let mut buf = [0.0f32; BLOCK * 2];
    while shared.playing.load(Ordering::Acquire) && out.len() / 2 < cap {
        renderer.process(&mut buf, 2);
        out.extend_from_slice(&buf);
    }
    if let Some(o) = offline {
        o.finish(renderer.take_plugins());
    }

    if let (Some((from, to)), Some(start)) = (range, start) {
        // 手前に描き出した分を捨て、範囲の分だけにする(自動停止で足りなければ無音で埋める)
        let skip = ((from - start) as usize * 2).min(out.len());
        out.drain(..skip);
        out.resize((to - from) as usize * 2, 0.0);
        return Ok(out);
    }

    // 末尾の無音を切り詰める(+0.5 秒の余白を残す)
    let last_audible = out
        .iter()
        .rposition(|s| s.abs() > 1e-4)
        .unwrap_or(out.len().saturating_sub(1));
    let keep = (last_audible / 2 + 1 + (0.5 * sample_rate) as usize) * 2;
    out.truncate(keep.min(out.len()));
    Ok(out)
}

/// トラックの音源(とエフェクト)で 1 音だけ鳴らした音(モノラル)。音色の解析・比較用。
/// トラックの他のクリップ・オートメーション・音量・パン、マスターのエフェクトは使わない。
/// `seconds` は鍵盤を押している長さで、余韻の分だけ後ろに伸びる(最大 +2 秒程度)。
pub fn render_track_note(
    project: &Project,
    track_id: &glaux_core::TrackId,
    pitch: u8,
    velocity: u8,
    seconds: f64,
    sample_rate: f64,
    bank: &crate::data::SampleBank,
) -> Result<Vec<f32>, ExportError> {
    use glaux_core::{
        Clip, ClipContent, ClipId, Note, NoteId, TempoEvent, TempoMap, Tick, TrackKind,
    };
    let track = project
        .track(track_id)
        .filter(|t| t.kind == TrackKind::Midi)
        .ok_or(ExportError::Empty)?;
    let mut p = project.clone();
    // 120 BPM 固定(1 秒 = 1920 tick)
    p.tempo_map = TempoMap::new(vec![TempoEvent {
        tick: Tick::ZERO,
        bpm: 120.0,
    }])
    .map_err(|_| ExportError::Empty)?;
    p.master.effects.clear();
    p.master.fx_links = None;
    p.master.automation.clear();
    p.master.volume_db = 0.0;
    let mut t = track.clone();
    t.automation.clear();
    t.mute = false;
    t.solo = false;
    t.volume_db = 0.0;
    t.pan = 0.0;
    let dur = ((seconds.max(0.05) * 1920.0) as u64).max(1);
    let mut clip = Clip::new_midi(ClipId::new(), "test", Tick::ZERO, Tick(dur + 1920));
    if let ClipContent::Midi { notes, .. } = &mut clip.content {
        notes.push(Note {
            id: NoteId::new(),
            pos: Tick::ZERO,
            dur: Tick(dur),
            pitch: pitch.min(127),
            vel: velocity.clamp(1, 127),
            articulation: Default::default(),
            pitch_curve: vec![],
            glide_ms: None,
        });
    }
    t.clips = vec![clip];
    p.tracks = vec![t];
    let stereo = render_project(&p, sample_rate, bank)?;
    Ok(stereo
        .chunks(2)
        .map(|c| (c[0] + c.get(1).copied().unwrap_or(c[0])) * 0.5)
        .collect())
}

/// プロジェクトを 16bit ステレオ WAV に書き出す。返り値は書き出した秒数。
pub fn export_wav(
    project: &Project,
    path: &Path,
    sample_rate: f64,
    bank: &crate::data::SampleBank,
) -> Result<f64, ExportError> {
    let samples = render_project(project, sample_rate, bank)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let spec = hound::WavSpec {
        channels: 2,
        sample_rate: sample_rate as u32,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer = hound::WavWriter::create(path, spec)?;
    let mut shaper = Shaper::new(sample_rate);
    for (i, s) in samples.iter().enumerate() {
        writer.write_sample(shaper.quantize(*s, i % 2))?;
    }
    writer.finalize()?;
    Ok(samples.len() as f64 / 2.0 / sample_rate)
}

// ---- 書き出しの選択肢 ------------------------------------------------------

/// 書き出しの設定。
#[derive(Clone, Debug, PartialEq)]
pub struct ExportOptions {
    /// 44100 か 48000
    pub sample_rate: u32,
    /// 16 / 24(整数、ディザあり)/ 32(浮動小数)
    pub bits: u16,
    /// 秒の範囲 [開始, 終了)。None で曲全体
    pub range_secs: Option<(f64, f64)>,
    /// 音量の目標(統合ラウドネス LUFS)。None でそのまま
    pub target_lufs: Option<f64>,
    /// 音量を合わせるときのピークの上限(dBTP = True Peak)。超える所はリミッタで抑える
    pub ceiling_db: f64,
    /// 16bit のとき、量子化の雑音を耳につきにくい帯域へ寄せる(ノイズシェーピング)
    pub noise_shaping: bool,
    /// ファイルの形式(FLAC は 16 / 24bit のみ)
    pub format: AudioFormat,
}

impl Default for ExportOptions {
    fn default() -> Self {
        ExportOptions {
            sample_rate: 48_000,
            bits: 16,
            range_secs: None,
            target_lufs: None,
            ceiling_db: -1.0,
            noise_shaping: true,
            format: AudioFormat::Wav,
        }
    }
}

/// 書き出しの結果(書き出した音の測定値)。
#[derive(Clone, Debug, serde::Serialize)]
pub struct ExportReport {
    pub seconds: f64,
    /// 統合ラウドネス(LUFS)
    pub lufs: f64,
    /// サンプルのピーク(dBFS)
    pub peak_db: f64,
    /// True Peak(dBTP。サンプルの間の山も含む)
    pub true_peak_db: f64,
    /// PLR(True Peak − 統合ラウドネス、dB)。小さいほど潰れている
    pub plr_db: f64,
    /// 音量を合わせるために掛けたゲイン(dB)
    pub gain_db: f64,
    /// リミッタで最も下げた量(dB。0 なら掛かっていない)。AES TD1008 は 1 dB 程度までを出発点に勧めている
    pub limiter_db: f64,
    /// 配信サービスで再生されたときの音量の調整の予測
    pub streaming: Vec<crate::loudness::StreamingPreview>,
}

fn db(a: f64) -> f64 {
    20.0 * a.max(1e-9).log10()
}

/// 範囲の指定があればその範囲、無ければ全体を描き出す。`master_clip` が false ならマスターのクリップ防止を通さない
pub fn render(
    project: &Project,
    sample_rate: f64,
    bank: &crate::data::SampleBank,
    range_secs: Option<(f64, f64)>,
    master_clip: bool,
) -> Result<Vec<f32>, ExportError> {
    let range = range_secs.map(|(a, b)| {
        let from = (a.max(0.0) * sample_rate) as u64;
        (from, ((b * sample_rate) as u64).max(from))
    });
    render_inner(project, sample_rate, bank, range, master_clip)
}

/// True Peak を `ceiling_db`(dBTP)以下に抑える先読み付きのリミッタ(書き出し用。全体を先に持っているので先読みできる)。
/// ピークの 5ms 前からゲインを下げ始め、過ぎたら 80ms ほどで戻す。ゲインの変化で波形が変わり
/// サンプルの間の山がわずかに残ることがあるので、測り直して超えていればもう一度かける。
/// 最後にサンプル値でも上限でクリップして保証する。最も下げた量(dB、正の値)を返す
pub fn limit_peaks(stereo: &mut [f32], sample_rate: f64, ceiling_db: f64) -> f64 {
    let mut reduced = limit_once(stereo, sample_rate, ceiling_db);
    let over = crate::loudness::true_peak_db(stereo) - ceiling_db;
    if over > 0.02 {
        reduced += limit_once(stereo, sample_rate, ceiling_db - over - 0.05);
    }
    reduced
}

fn limit_once(stereo: &mut [f32], sample_rate: f64, ceiling_db: f64) -> f64 {
    let ceil = 10f32.powf(ceiling_db as f32 / 20.0);
    let n = stereo.len() / 2;
    if n == 0 {
        return 0.0;
    }
    // 各フレームで必要なゲイン(True Peak で判定)
    let need: Vec<f32> = crate::loudness::true_peak_frames(stereo)
        .into_iter()
        .map(|p| if p > ceil { ceil / p } else { 1.0 })
        .collect();
    let look = ((0.005 * sample_rate) as usize).max(1);
    // 先読みの窓の最小値(単調な両端キューで O(n))
    let mut env = vec![1.0f32; n];
    let mut dq: std::collections::VecDeque<usize> = std::collections::VecDeque::new();
    for j in (0..n).rev() {
        while dq.back().is_some_and(|&k| need[k] >= need[j]) {
            dq.pop_back();
        }
        dq.push_back(j);
        while dq.front().is_some_and(|&k| k > j + look) {
            dq.pop_front();
        }
        env[j] = need[*dq.front().expect("入れた直後")];
    }
    // 戻り(リリース)は前向き、下げ始め(アタック)は後ろ向きに滑らかに
    let rel = 1.0 - (-1.0 / (0.08 * sample_rate)).exp() as f32;
    let att = 1.0 - (-1.0 / (look as f64 / 3.0)).exp() as f32;
    for i in 1..n {
        env[i] = env[i].min(env[i - 1] + (1.0 - env[i - 1]) * rel);
    }
    for i in (0..n - 1).rev() {
        env[i] = env[i].min(env[i + 1] + (1.0 - env[i + 1]) * att);
    }
    for i in 0..n {
        for c in 0..2 {
            let v = &mut stereo[i * 2 + c];
            *v = (*v * env[i]).clamp(-ceil, ceil);
        }
    }
    let min = env.iter().copied().fold(1.0f32, f32::min);
    -20.0 * (min as f64).max(1e-9).log10()
}

/// 設定に従って描き出し、音量を合わせて(指定があれば)WAV に書く。
pub fn export_audio(
    project: &Project,
    path: &Path,
    opts: &ExportOptions,
    bank: &crate::data::SampleBank,
) -> Result<ExportReport, ExportError> {
    let sr = opts.sample_rate as f64;
    let mut stereo = render(
        project,
        sr,
        bank,
        opts.range_secs,
        opts.target_lufs.is_none(),
    )?;
    let mut gain_db = 0.0;
    let mut limiter_db = 0.0;
    if let Some(target) = opts.target_lufs {
        // ラウドネスは 48kHz の係数で測る(サンプルレートが違えば 48kHz で描き出して測る)
        let measured = if opts.sample_rate == 48_000 {
            crate::analyze::integrated_lufs(&stereo)
        } else {
            let m = render(project, 48_000.0, bank, opts.range_secs, false)?;
            crate::analyze::integrated_lufs(&m)
        };
        if measured.is_finite() {
            gain_db = target - measured;
            let g = 10f32.powf(gain_db as f32 / 20.0);
            stereo.iter_mut().for_each(|v| *v *= g);
        }
        limiter_db = limit_peaks(&mut stereo, sr, opts.ceiling_db);
    }
    write_audio(
        path,
        &stereo,
        opts.sample_rate,
        opts.bits,
        opts.noise_shaping,
        opts.format,
    )?;
    let lufs = if opts.sample_rate == 48_000 {
        crate::analyze::integrated_lufs(&stereo)
    } else {
        // 48kHz 以外は ebur128(サンプルレートに合わせた K 特性)で測る
        ebur128::EbuR128::new(2, opts.sample_rate, ebur128::Mode::I)
            .ok()
            .and_then(|mut m| {
                m.add_frames_f32(&stereo).ok()?;
                m.loudness_global().ok()
            })
            .filter(|v| v.is_finite())
            .unwrap_or(f64::NAN)
    };
    let peak = stereo.iter().fold(0.0f32, |m, v| m.max(v.abs())) as f64;
    let true_peak = crate::loudness::true_peak_db(&stereo);
    let round = |v: f64| (v * 10.0).round() / 10.0;
    Ok(ExportReport {
        seconds: stereo.len() as f64 / 2.0 / sr,
        lufs: round(lufs),
        peak_db: round(db(peak)),
        true_peak_db: round(true_peak),
        plr_db: if lufs.is_finite() {
            round(true_peak - lufs)
        } else {
            f64::NAN
        },
        gain_db: round(gain_db),
        limiter_db: round(limiter_db),
        streaming: crate::loudness::streaming_previews(lufs, true_peak),
    })
}

/// ステレオ・インターリーブを WAV に書く(16 / 24 はディザ付きの整数、32 は浮動小数)。
/// 16bit はノイズシェーピング付き([`write_wav_with`] で切れる)
pub fn write_wav(
    path: &Path,
    stereo: &[f32],
    sample_rate: u32,
    bits: u16,
) -> Result<(), ExportError> {
    write_wav_with(path, stereo, sample_rate, bits, true)
}

/// [`write_wav`] と同じ。`noise_shaping` が false なら 16bit も TPDF ディザだけ
pub fn write_wav_with(
    path: &Path,
    stereo: &[f32],
    sample_rate: u32,
    bits: u16,
    noise_shaping: bool,
) -> Result<(), ExportError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let float = bits == 32;
    let spec = hound::WavSpec {
        channels: 2,
        sample_rate,
        bits_per_sample: if float {
            32
        } else if bits == 24 {
            24
        } else {
            16
        },
        sample_format: if float {
            hound::SampleFormat::Float
        } else {
            hound::SampleFormat::Int
        },
    };
    let mut w = hound::WavWriter::create(path, spec)?;
    if float {
        for &s in stereo {
            w.write_sample(s)?;
        }
    } else {
        for v in to_ints(stereo, sample_rate, bits, noise_shaping) {
            w.write_sample(v)?;
        }
    }
    w.finalize()?;
    Ok(())
}

/// 形式を選んで書く([`write_wav_with`] か FLAC)
pub fn write_audio(
    path: &Path,
    stereo: &[f32],
    sample_rate: u32,
    bits: u16,
    noise_shaping: bool,
    format: AudioFormat,
) -> Result<(), ExportError> {
    match format {
        AudioFormat::Wav => write_wav_with(path, stereo, sample_rate, bits, noise_shaping),
        AudioFormat::Flac => write_flac(path, stereo, sample_rate, bits, noise_shaping),
    }
}

/// 16 / 24bit の整数にする(ディザ付き。16bit は `noise_shaping` でノイズシェーピング)。WAV と FLAC で共通
fn to_ints(stereo: &[f32], sample_rate: u32, bits: u16, noise_shaping: bool) -> Vec<i32> {
    let mut dither = Tpdf::new();
    let mut shaper = (bits == 16 && noise_shaping).then(|| Shaper::new(sample_rate as f64));
    stereo
        .iter()
        .enumerate()
        .map(|(i, &s)| {
            if bits == 24 {
                let full = 8_388_607.0f32;
                let v = (s.clamp(-1.0, 1.0) * full + dither.next()).round();
                v.clamp(-full - 1.0, full) as i32
            } else if let Some(sh) = shaper.as_mut() {
                sh.quantize(s, i % 2) as i32
            } else {
                to_i16_dithered(s, &mut dither) as i32
            }
        })
        .collect()
}

/// ステレオ・インターリーブを FLAC(可逆圧縮)で書く。16 / 24bit のみ
pub fn write_flac(
    path: &Path,
    stereo: &[f32],
    sample_rate: u32,
    bits: u16,
    noise_shaping: bool,
) -> Result<(), ExportError> {
    use flacenc::component::BitRepr;
    use flacenc::error::Verify;
    if !matches!(bits, 16 | 24) {
        return Err(ExportError::Flac(
            "FLAC は 16 か 24bit で書き出してください".to_owned(),
        ));
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let ints = to_ints(stereo, sample_rate, bits, noise_shaping);
    let config = flacenc::config::Encoder::default()
        .into_verified()
        .map_err(|(_, e)| ExportError::Flac(format!("{e:?}")))?;
    let source =
        flacenc::source::MemSource::from_samples(&ints, 2, bits as usize, sample_rate as usize);
    let stream = flacenc::encode_with_fixed_block_size(&config, source, config.block_size)
        .map_err(|e| ExportError::Flac(format!("{e:?}")))?;
    let mut sink = flacenc::bitsink::ByteSink::new();
    stream
        .write(&mut sink)
        .map_err(|e| ExportError::Flac(format!("{e:?}")))?;
    let mut bytes = sink.as_slice().to_vec();
    // STREAMINFO の最小ブロック長は「最後のブロックを除く」(仕様)。flacenc は最後の短いブロックの長さを
    // 入れるため、最小と最大が違う = 可変ブロック長のストリームと読まれ、symphonia(Glaux の取り込みも)が
    // 読めなかった。固定長で書いているので最大と同じにする(先頭 4 バイト "fLaC" + ブロックの頭 4 バイトの後)
    if bytes.len() >= 12 && &bytes[..4] == b"fLaC" && bytes[4] & 0x7f == 0 {
        let max = [bytes[10], bytes[11]];
        bytes[8..10].copy_from_slice(&max);
    }
    std::fs::write(path, bytes)?;
    Ok(())
}

/// TPDF ディザ(三角分布の雑音)。16bit に丸めるときの量子化の歪みを、耳につきにくい
/// 一様な雑音に変える(フェードアウトや静かな余韻が「ジリジリ」しない)
struct Tpdf(u32);

impl Tpdf {
    fn new() -> Self {
        Tpdf(0x9E37_79B9)
    }

    /// -1..1 LSB の三角分布(一様乱数 2 つの差)
    fn next(&mut self) -> f32 {
        let mut u = || {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 17;
            self.0 ^= self.0 << 5;
            self.0 as f32 / u32::MAX as f32
        };
        u() - u()
    }
}

/// ノイズシェーピングの段数(Wannamaker と同じ 9 次)
const SHAPE_ORDER: usize = 9;

/// ノイズシェーピングの係数を設計する(サンプルレートごと)。
///
/// 量子化の雑音の形 NTF(z) = 1 + Σ a_k z^-k の、聞こえやすさで重み付けした雑音の大きさ
/// ∫ |NTF(ω)|² W(ω) dω を最小にする 9 係数を求める。W は人が聞こえる最小の音(Terhardt の絶対閾)の逆で、
/// 耳が敏感な 2〜5kHz ほど重い。これは線形予測(LPC)と同じ形なので、W の自己相関から Levinson-Durbin で解け、
/// 答えは必ず最小位相(誤差の帰還が安定)になる。W の幅は 50dB で下を切る(聞こえにくい高域へ雑音を
/// 押しやりすぎない)。SoX の係数表(LGPL)は使わず、ここで作る
fn design_noise_shaping(sample_rate: f64) -> [f32; SHAPE_ORDER] {
    const M: usize = 4096;
    let ath = |f_hz: f64| -> f64 {
        let f = (f_hz / 1000.0).max(0.02);
        3.64 * f.powf(-0.8) - 6.5 * (-0.6 * (f - 3.3).powi(2)).exp() + 1e-3 * f.powi(4)
    };
    // 0..ナイキストの重み(聞こえやすさ)。最大から 50dB 下で切る
    let w_db: Vec<f64> = (0..M)
        .map(|k| -ath((k as f64 + 0.5) / M as f64 * sample_rate / 2.0))
        .collect();
    let top = w_db.iter().copied().fold(f64::MIN, f64::max);
    let w: Vec<f64> = w_db
        .iter()
        .map(|d| 10f64.powf(d.max(top - 50.0) / 10.0))
        .collect();
    // 自己相関 r[k] = ∫ W(ω) cos(kω) dω
    let r: Vec<f64> = (0..=SHAPE_ORDER)
        .map(|k| {
            w.iter()
                .enumerate()
                .map(|(i, wi)| {
                    let om = (i as f64 + 0.5) / M as f64 * std::f64::consts::PI;
                    wi * (k as f64 * om).cos()
                })
                .sum::<f64>()
        })
        .collect();
    // Levinson-Durbin
    let mut a = [0.0f64; SHAPE_ORDER + 1];
    a[0] = 1.0;
    let mut err = r[0];
    for m in 1..=SHAPE_ORDER {
        let acc: f64 = (1..m).map(|k| a[k] * r[m - k]).sum::<f64>() + r[m];
        let kappa = -acc / err;
        let prev = a;
        for k in 1..m {
            a[k] = prev[k] + kappa * prev[m - k];
        }
        a[m] = kappa;
        err *= 1.0 - kappa * kappa;
    }
    std::array::from_fn(|k| a[k + 1] as f32)
}

/// 16bit へのノイズシェーピング付きの量子化(TPDF ディザ + 誤差の帰還、左右別)
struct Shaper {
    coefs: [f32; SHAPE_ORDER],
    /// 直近の量子化誤差(左右、新しい順)
    err: [[f32; SHAPE_ORDER]; 2],
    dither: Tpdf,
}

impl Shaper {
    fn new(sample_rate: f64) -> Self {
        Shaper {
            coefs: design_noise_shaping(sample_rate),
            err: [[0.0; SHAPE_ORDER]; 2],
            dither: Tpdf::new(),
        }
    }

    /// 1 サンプルを 16bit に(`ch` は 0 = 左、1 = 右)
    fn quantize(&mut self, s: f32, ch: usize) -> i16 {
        let full = i16::MAX as f32;
        let e = &mut self.err[ch];
        let fb: f32 = self.coefs.iter().zip(e.iter()).map(|(a, x)| a * x).sum();
        let v = s.clamp(-1.0, 1.0) * full + fb;
        let q = (v + self.dither.next())
            .round()
            .clamp(i16::MIN as f32, i16::MAX as f32);
        // 誤差は大きく振り切れたとき(クリップ)に暴れないよう抑える
        let err = (q - v).clamp(-4.0, 4.0);
        e.copy_within(0..SHAPE_ORDER - 1, 1);
        e[0] = err;
        q as i16
    }
}

/// f32(-1..1)を 16bit に。ディザを足してから丸める(以前は切り捨て)
fn to_i16_dithered(s: f32, dither: &mut Tpdf) -> i16 {
    let scaled = s.clamp(-1.0, 1.0) * i16::MAX as f32 + dither.next();
    scaled.round().clamp(i16::MIN as f32, i16::MAX as f32) as i16
}

#[cfg(test)]
mod tests {
    use super::*;
    use glaux_core::{Clip, ClipContent, ClipId, Note, NoteId, Tick, Track, TrackId, TrackKind};

    /// 量子化の誤差(元の音との差、LSB)を、聞こえやすさ(絶対閾)で重み付けした雑音の大きさ(dB)と、重みなしの大きさ
    fn noise_db(err: &[f64], sr: f64) -> (f64, f64) {
        use rustfft::{num_complex::Complex, FftPlanner};
        let n = 8192;
        let fft = FftPlanner::<f64>::new().plan_fft_forward(n);
        let (mut weighted, mut plain) = (0.0, 0.0);
        for chunk in err.chunks_exact(n) {
            let mut buf: Vec<Complex<f64>> = chunk
                .iter()
                .enumerate()
                .map(|(i, v)| {
                    let w = 0.5 - 0.5 * (std::f64::consts::TAU * i as f64 / n as f64).cos();
                    Complex::new(v * w, 0.0)
                })
                .collect();
            fft.process(&mut buf);
            for (k, c) in buf[1..n / 2].iter().enumerate() {
                let f = ((k + 1) as f64 * sr / n as f64 / 1000.0).max(0.02);
                let ath =
                    3.64 * f.powf(-0.8) - 6.5 * (-0.6 * (f - 3.3).powi(2)).exp() + 1e-3 * f.powi(4);
                let p = c.norm_sqr();
                plain += p;
                weighted += p * 10f64.powf(-ath / 10.0);
            }
        }
        (10.0 * weighted.log10(), 10.0 * plain.log10())
    }

    #[test]
    fn noise_shaping_moves_noise_away_from_sensitive_bands() {
        for sr in [44_100.0f64, 48_000.0] {
            // 静かな(-60dBFS)1kHz の音を 16bit にしたときの誤差
            let x: Vec<f32> = (0..(sr as usize * 2))
                .map(|i| (i as f64 * 1000.0 * std::f64::consts::TAU / sr).sin() as f32 * 0.001)
                .collect();
            let mut shaper = Shaper::new(sr);
            let mut tpdf = Tpdf::new();
            let full = i16::MAX as f64;
            let shaped: Vec<f64> = x
                .iter()
                .map(|v| shaper.quantize(*v, 0) as f64 - *v as f64 * full)
                .collect();
            let plain: Vec<f64> = x
                .iter()
                .map(|v| to_i16_dithered(*v, &mut tpdf) as f64 - *v as f64 * full)
                .collect();
            let (ws, us) = noise_db(&shaped, sr);
            let (wp, up) = noise_db(&plain, sr);
            // 聞こえやすさで重み付けすると 10dB 以上静か(重みなしの雑音は増える)
            assert!(
                ws < wp - 10.0,
                "{sr}: 重み付き シェーピング {ws:.1} / TPDF {wp:.1}"
            );
            assert!(us > up, "{sr}: 重みなしの雑音は増える {us:.1} / {up:.1}");
        }
        // 大きな音(振り切れる所を含む)でも暴れない
        let mut shaper = Shaper::new(48_000.0);
        for i in 0..48_000 {
            let v = (i as f32 * 0.05).sin() * 1.2;
            let q = shaper.quantize(v, 1) as f32 / i16::MAX as f32;
            assert!((q - v.clamp(-1.0, 1.0)).abs() < 0.01, "{i}: {q} / {v}");
        }
    }

    #[test]
    fn limiter_keeps_peaks_under_the_ceiling_and_leaves_quiet_parts() {
        let sr = 48_000.0;
        // 静かな音の途中に大きな山
        let mut x: Vec<f32> = (0..48_000)
            .flat_map(|i| {
                let t = i as f32 / 48_000.0;
                let a = if (0.5..0.52).contains(&t) { 1.8 } else { 0.2 };
                let v = a * (t * 440.0 * std::f32::consts::TAU).sin();
                [v, v]
            })
            .collect();
        limit_peaks(&mut x, sr, -1.0);
        let ceil = 10f32.powf(-1.0 / 20.0);
        assert!(x.iter().all(|v| v.abs() <= ceil + 1e-6));
        // サンプルの間の山(True Peak)も上限を超えない
        let tp = crate::loudness::true_peak_db(&x);
        assert!(tp <= -1.0 + 0.05, "True Peak も上限以下: {tp:.2} dBTP");
        // 山から離れた静かな所はそのまま
        let quiet = x[2 * 10_000..2 * 10_100]
            .iter()
            .fold(0.0f32, |m, v| m.max(v.abs()));
        assert!((quiet - 0.2).abs() < 0.01, "{quiet}");
    }

    #[test]
    fn export_hits_the_loudness_target_and_the_format() {
        let tmp = tempfile::tempdir().unwrap();
        let project = test_project();
        let path = tmp.path().join("out.wav");
        let opts = ExportOptions {
            sample_rate: 44_100,
            bits: 24,
            target_lufs: Some(-14.0),
            ..Default::default()
        };
        let r = export_audio(&project, &path, &opts, &Default::default()).unwrap();
        let reader = hound::WavReader::open(&path).unwrap();
        assert_eq!(reader.spec().sample_rate, 44_100);
        assert_eq!(reader.spec().bits_per_sample, 24);
        assert!(r.peak_db <= -0.9, "{r:?}");
        // True Peak も上限(-1 dBTP)以下で、44.1kHz でもラウドネスを測って配信の予測を返す
        assert!(r.true_peak_db <= -0.9, "{r:?}");
        assert!((r.lufs + 14.0).abs() < 1.0, "{r:?}");
        assert!(r.streaming.iter().any(|s| s.service == "Spotify"));
        // 48kHz で書き出せばラウドネスを測って返す。目標に ±1 LU
        let path48 = tmp.path().join("out48.wav");
        let r = export_audio(
            &project,
            &path48,
            &ExportOptions {
                target_lufs: Some(-14.0),
                ..Default::default()
            },
            &Default::default(),
        )
        .unwrap();
        assert!((r.lufs + 14.0).abs() < 1.0, "{r:?}");
    }

    /// FLAC を symphonia で読み戻す(左右インターリーブの整数と、1 サンプルのビット数)
    fn read_flac(path: &Path) -> (Vec<i32>, u32, u32) {
        use symphonia::core::audio::SampleBuffer;
        use symphonia::core::codecs::DecoderOptions;
        use symphonia::core::formats::FormatOptions;
        use symphonia::core::io::MediaSourceStream;
        use symphonia::core::meta::MetadataOptions;
        use symphonia::core::probe::Hint;
        let file = std::fs::File::open(path).unwrap();
        let mss = MediaSourceStream::new(Box::new(file), Default::default());
        let mut hint = Hint::new();
        hint.with_extension("flac");
        let probed = symphonia::default::get_probe()
            .format(
                &hint,
                mss,
                &FormatOptions::default(),
                &MetadataOptions::default(),
            )
            .unwrap();
        let mut format = probed.format;
        let track = format.default_track().unwrap();
        let params = track.codec_params.clone();
        let mut dec = symphonia::default::get_codecs()
            .make(&params, &DecoderOptions::default())
            .unwrap();
        let mut out = Vec::new();
        while let Ok(packet) = format.next_packet() {
            let buf = dec.decode(&packet).unwrap();
            let mut sb = SampleBuffer::<i32>::new(buf.capacity() as u64, *buf.spec());
            sb.copy_interleaved_ref(buf);
            out.extend_from_slice(sb.samples());
        }
        (
            out,
            params.sample_rate.unwrap(),
            params.bits_per_sample.unwrap(),
        )
    }

    #[test]
    fn flac_is_lossless_and_smaller_than_wav() {
        let tmp = tempfile::tempdir().unwrap();
        let n = 48_000;
        let stereo: Vec<f32> = (0..n)
            .flat_map(|i| {
                let t = i as f32 / 48_000.0;
                let x = (t * 440.0 * std::f32::consts::TAU).sin() * 0.5 * (-t * 2.0).exp();
                [x, x * 0.7]
            })
            .collect();
        for bits in [16u16, 24] {
            let path = tmp.path().join(format!("a{bits}.flac"));
            write_audio(&path, &stereo, 44_100, bits, true, AudioFormat::Flac).unwrap();
            let (got, sr, b) = read_flac(&path);
            assert_eq!((sr, b), (44_100, bits as u32));
            // 読み戻した整数が、書く前の整数とぴったり同じ(可逆)。symphonia は 32bit に左寄せで返す
            let want = to_ints(&stereo, 44_100, bits, true);
            let shift = 32 - bits as u32;
            assert_eq!(got.len(), want.len());
            assert!(got.iter().zip(&want).all(|(g, w)| g >> shift == *w));
            let wav = tmp.path().join(format!("a{bits}.wav"));
            write_audio(&wav, &stereo, 44_100, bits, true, AudioFormat::Wav).unwrap();
            let (fs, ws) = (
                std::fs::metadata(&path).unwrap().len(),
                std::fs::metadata(&wav).unwrap().len(),
            );
            assert!(fs * 2 < ws, "{bits}bit: FLAC {fs} / WAV {ws}");
        }
        // 32bit(浮動小数)は FLAC にできない
        assert!(write_audio(
            &tmp.path().join("x.flac"),
            &stereo,
            48_000,
            32,
            false,
            AudioFormat::Flac
        )
        .is_err());
    }

    #[test]
    fn dither_turns_a_tiny_signal_into_noise_instead_of_silence_or_steps() {
        // 0.3 LSB のサイン波: 切り捨てなら全部 0(消える)。ディザなら平均として形が残る
        let mut d = Tpdf::new();
        let n = 48_000;
        let amp = 0.3 / i16::MAX as f32;
        let mut acc = 0.0f64;
        for i in 0..n {
            let x = amp * (i as f32 * 0.01).sin();
            let q = to_i16_dithered(x, &mut d) as f64;
            acc += q * (i as f32 * 0.01).sin() as f64;
        }
        // 相関が正(信号の形が雑音の中に残っている)
        assert!(acc / n as f64 > 0.05, "{}", acc / n as f64);
        // 大きな音はほぼそのまま(±1 LSB)
        let mut d = Tpdf::new();
        assert!((to_i16_dithered(0.5, &mut d) as i32 - 16_383).abs() <= 1);
        assert_eq!(to_i16_dithered(2.0, &mut d), i16::MAX);
    }

    fn test_project() -> Project {
        let mut project = Project::new("Export");
        let mut track = Track::new(TrackId::new(), "T", TrackKind::Midi);
        let mut clip = Clip::new_midi(ClipId::new(), "c", Tick(0), Tick(1920));
        if let ClipContent::Midi { notes, .. } = &mut clip.content {
            notes.push(Note {
                articulation: Default::default(),
                pitch_curve: vec![],
                id: NoteId::new(),
                pos: Tick(0),
                dur: Tick(960),
                pitch: 60,
                vel: 100,
                glide_ms: None,
            });
        }
        track.clips.push(clip);
        project.tracks.push(track);
        project
    }

    #[test]
    fn renders_project_offline() {
        let samples = render_project(&test_project(), 48_000.0, &Default::default()).unwrap();
        // 0.5 秒のノート + 余韻。ステレオなので偶数長
        assert!(samples.len().is_multiple_of(2));
        assert!(samples.len() as f64 / 2.0 / 48_000.0 > 0.5);
        let rms = (samples.iter().map(|s| s * s).sum::<f32>() / samples.len() as f32).sqrt();
        assert!(rms > 0.01, "音が入っているはず");
    }

    #[test]
    fn writes_valid_wav() {
        let dir = std::env::temp_dir().join("glaux-export-test");
        let path = dir.join("out.wav");
        let seconds = export_wav(&test_project(), &path, 48_000.0, &Default::default()).unwrap();
        assert!(seconds > 0.5);

        let reader = hound::WavReader::open(&path).unwrap();
        let spec = reader.spec();
        assert_eq!(spec.channels, 2);
        assert_eq!(spec.sample_rate, 48_000);
        assert_eq!(reader.duration() as f64 / 48_000.0, seconds);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn empty_project_is_an_error() {
        let project = Project::new("Empty");
        assert!(matches!(
            render_project(&project, 48_000.0, &Default::default()),
            Err(ExportError::Empty)
        ));
    }
}
