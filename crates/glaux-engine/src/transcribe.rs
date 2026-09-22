//! 単旋律の譜起こし(音声 → ノート列)。「鼻歌を MIDI に」が主な用途。
//!
//! 手順:
//! 1. 10ms ごとのフレームで YIN(累積平均正規化差分)により基本周波数を推定
//! 2. 音量(RMS)と YIN の明瞭度で有声/無声を判定
//! 3. ピッチ列に中央値フィルタを掛けてオクターブ跳びを潰す
//! 4. 「半音が変わって 3 フレーム安定」「音量の立ち上がり」「無声区間」で
//!    ノートを区切り、短すぎるものは捨てる
//! 5. テンポマップで秒 → tick に換算し、必要ならグリッドにクオンタイズ
//!
//! 和音・複数楽器は対象外(単旋律のみ)。精度はきれいな単音源で高く、
//! 出力は人間か AI が整える前提。

use glaux_core::{Articulation, Note, NoteId, TempoMap, Tick};
use glaux_dsp::SampleData;

#[derive(Clone, Copy, Debug)]
pub struct TranscribeOptions {
    /// 探索する基本周波数の範囲(Hz)。鼻歌・歌なら 70〜1000 で十分
    pub pitch_lo_hz: f32,
    pub pitch_hi_hz: f32,
    /// これより短いノートは捨てる(ms)
    pub min_note_ms: f32,
    /// 有声とみなす YIN の明瞭度(1 - d')の下限
    pub min_clarity: f32,
}

impl Default for TranscribeOptions {
    fn default() -> Self {
        TranscribeOptions {
            pitch_lo_hz: 70.0,
            pitch_hi_hz: 1000.0,
            min_note_ms: 80.0,
            min_clarity: 0.5,
        }
    }
}

/// 秒単位のノート(tick 化の前)。
#[derive(Clone, Debug, PartialEq)]
pub struct TranscribedNote {
    pub start_sec: f64,
    pub end_sec: f64,
    pub pitch: u8,
    pub vel: u8,
}

/// 1 フレームの解析結果。
#[derive(Clone, Copy, Debug)]
struct Frame {
    /// MIDI ノート番号(小数)。無声なら None
    midi: Option<f32>,
    rms_db: f32,
}

const HOP_SEC: f32 = 0.010;
const YIN_THRESHOLD: f32 = 0.15;

/// YIN で 1 フレームの基本周波数を推定する。戻り値は (周波数, 明瞭度)。
fn yin_pitch(buf: &[f32], sr: f32, tau_min: usize, tau_max: usize) -> Option<(f32, f32)> {
    let n = buf.len().saturating_sub(tau_max);
    if n < 64 || tau_max <= tau_min {
        return None;
    }
    // 差分関数 d(tau)
    let mut d = vec![0.0f32; tau_max + 1];
    for (tau, dt) in d.iter_mut().enumerate().skip(1) {
        let mut acc = 0.0f32;
        for j in 0..n {
            let diff = buf[j] - buf[j + tau];
            acc += diff * diff;
        }
        *dt = acc;
    }
    // 累積平均正規化差分 d'(tau)
    let mut cmnd = vec![1.0f32; tau_max + 1];
    let mut running = 0.0f32;
    for tau in 1..=tau_max {
        running += d[tau];
        cmnd[tau] = if running > 0.0 {
            d[tau] * tau as f32 / running
        } else {
            1.0
        };
    }
    // しきい値を下回る最初の谷(絶対しきい値法)
    let mut tau = tau_min;
    let mut found = None;
    while tau < tau_max {
        if cmnd[tau] < YIN_THRESHOLD {
            while tau + 1 < tau_max && cmnd[tau + 1] < cmnd[tau] {
                tau += 1;
            }
            found = Some(tau);
            break;
        }
        tau += 1;
    }
    // 見つからなければ範囲内の最小値(明瞭度は低くなる)
    let tau = found.unwrap_or_else(|| {
        (tau_min..tau_max)
            .min_by(|a, b| {
                cmnd[*a]
                    .partial_cmp(&cmnd[*b])
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .unwrap_or(tau_min)
    });
    // 放物線補間でサブサンプル精度に
    let refined = if tau > 0 && tau < tau_max {
        let (a, b, c) = (cmnd[tau - 1], cmnd[tau], cmnd[tau + 1]);
        let denom = a - 2.0 * b + c;
        if denom.abs() > 1e-9 {
            tau as f32 + 0.5 * (a - c) / denom
        } else {
            tau as f32
        }
    } else {
        tau as f32
    };
    let clarity = (1.0 - cmnd[tau]).clamp(0.0, 1.0);
    Some((sr / refined, clarity))
}

fn analyze_frames(data: &SampleData, opts: &TranscribeOptions) -> (Vec<Frame>, f32) {
    let sr = data.sample_rate;
    let hop = (HOP_SEC * sr).round().max(1.0) as usize;
    let tau_min = (sr / opts.pitch_hi_hz).floor().max(2.0) as usize;
    let tau_max = (sr / opts.pitch_lo_hz).ceil() as usize;
    // 積分窓は最低周期の 2 倍(低い声でも 2 周期入る)
    let win = (tau_max * 2).clamp(1024, 4096);
    let need = win + tau_max;
    let x = &data.frames;
    let mut frames = Vec::new();
    let mut peak_db = -120.0f32;
    let mut pos = 0usize;
    while pos + need <= x.len() {
        let buf = &x[pos..pos + need];
        let rms = (buf[..win].iter().map(|s| s * s).sum::<f32>() / win as f32).sqrt();
        let rms_db = 20.0 * rms.max(1e-9).log10();
        peak_db = peak_db.max(rms_db);
        let midi = yin_pitch(buf, sr, tau_min, tau_max).and_then(|(f, clarity)| {
            if clarity >= opts.min_clarity && f >= opts.pitch_lo_hz && f <= opts.pitch_hi_hz {
                Some(69.0 + 12.0 * (f / 440.0).log2())
            } else {
                None
            }
        });
        frames.push(Frame { midi, rms_db });
        pos += hop;
    }
    (frames, peak_db)
}

/// 中央値フィルタ(窓 5)。無声フレームは無視して両隣の有声値で埋めない(境界を保つ)。
fn median_filter(frames: &mut [Frame]) {
    let src: Vec<Option<f32>> = frames.iter().map(|f| f.midi).collect();
    for i in 0..frames.len() {
        if src[i].is_none() {
            continue;
        }
        let lo = i.saturating_sub(2);
        let hi = (i + 3).min(src.len());
        let mut v: Vec<f32> = src[lo..hi].iter().filter_map(|m| *m).collect();
        v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        frames[i].midi = Some(v[v.len() / 2]);
    }
}

/// 音声(モノラル)を単旋律のノート列に起こす。
pub fn transcribe_mono(data: &SampleData, opts: &TranscribeOptions) -> Vec<TranscribedNote> {
    let (mut frames, peak_db) = analyze_frames(data, opts);
    if frames.is_empty() {
        return vec![];
    }
    // 有声判定に音量条件を加える(最大から -40dB 以内、かつ -60dBFS 以上)
    let floor_db = (peak_db - 40.0).max(-60.0);
    for f in &mut frames {
        if f.rms_db < floor_db {
            f.midi = None;
        }
    }
    median_filter(&mut frames);

    let hop = HOP_SEC as f64;
    let min_frames = ((opts.min_note_ms / 1000.0) / HOP_SEC).round().max(1.0) as usize;
    const STABLE: usize = 3; // 半音が変わったと認める安定フレーム数
    const GAP: usize = 2; // 無声がこれ以上続いたらノート終了

    let mut notes = Vec::new();
    let mut cur: Option<(usize, u8, Vec<f32>, Vec<f32>)> = None; // (開始 frame, pitch, midi 列, dB 列)
    let mut gap = 0usize;

    let finish = |cur: &mut Option<(usize, u8, Vec<f32>, Vec<f32>)>,
                  end_frame: usize,
                  notes: &mut Vec<TranscribedNote>| {
        if let Some((start, pitch, _, dbs)) = cur.take() {
            if end_frame.saturating_sub(start) >= min_frames {
                let mean_db = dbs.iter().sum::<f32>() / dbs.len().max(1) as f32;
                // -40dB → 50、-10dB → 110 の線形マップ
                let vel = (50.0 + (mean_db + 40.0) * 2.0).clamp(30.0, 120.0) as u8;
                notes.push(TranscribedNote {
                    start_sec: start as f64 * hop,
                    end_sec: end_frame as f64 * hop,
                    pitch,
                    vel,
                });
            }
        }
    };

    let rounded = |m: f32| m.round().clamp(0.0, 127.0) as u8;
    for i in 0..frames.len() {
        let f = frames[i];
        match f.midi {
            None => {
                gap += 1;
                if gap >= GAP {
                    finish(&mut cur, i + 1 - gap, &mut notes);
                }
            }
            Some(m) => {
                gap = 0;
                let p = rounded(m);
                match &mut cur {
                    None => cur = Some((i, p, vec![m], vec![f.rms_db])),
                    Some((start, pitch, ms, dbs)) => {
                        // 音量の立ち上がり(同じ音程の再アタック "タタタ")
                        let onset = i >= 3
                            && frames[i - 3].midi.is_some()
                            && f.rms_db - frames[i - 3].rms_db > 8.0
                            && i - *start >= min_frames;
                        // 半音の変化が STABLE フレーム続くか
                        let changed = p != *pitch
                            && (1..STABLE).all(|k| {
                                frames
                                    .get(i + k)
                                    .and_then(|g| g.midi)
                                    .is_some_and(|mm| rounded(mm) == p)
                            });
                        if onset || changed {
                            finish(&mut cur, i, &mut notes);
                            cur = Some((i, p, vec![m], vec![f.rms_db]));
                        } else {
                            ms.push(m);
                            dbs.push(f.rms_db);
                        }
                    }
                }
            }
        }
    }
    let n = frames.len();
    finish(&mut cur, n.saturating_sub(gap.min(n)), &mut notes);
    notes
}

/// 秒単位のノートをクリップ相対の tick に換算する。
/// `quantize_ticks` > 0 なら開始位置と長さをそのグリッドに丸める(重なりは詰める)。
pub fn to_clip_notes(
    notes: &[TranscribedNote],
    tempo: &TempoMap,
    clip_start: Tick,
    clip_len: Tick,
    quantize_ticks: u64,
) -> Vec<Note> {
    let base_sec = tempo.tick_to_seconds(clip_start);
    let mut out: Vec<Note> = Vec::new();
    for n in notes {
        let abs_s = tempo.seconds_to_tick(base_sec + n.start_sec).0;
        let abs_e = tempo.seconds_to_tick(base_sec + n.end_sec).0;
        let mut pos = abs_s.saturating_sub(clip_start.0);
        let mut end = abs_e.saturating_sub(clip_start.0);
        if quantize_ticks > 0 {
            let q = quantize_ticks;
            pos = (pos as f64 / q as f64).round() as u64 * q;
            end = ((end as f64 / q as f64).round() as u64 * q).max(pos + q);
        }
        if pos >= clip_len.0 {
            continue;
        }
        let end = end.min(clip_len.0);
        if end <= pos {
            continue;
        }
        // 前のノートと重なったら前を詰める(同じ開始なら前を捨てる)
        if let Some(prev) = out.last_mut() {
            if prev.pos.0 == pos {
                out.pop();
            } else if prev.pos.0 + prev.dur.0 > pos {
                prev.dur = Tick(pos - prev.pos.0);
            }
        }
        out.push(Note {
            id: NoteId::new(),
            pos: Tick(pos),
            dur: Tick(end - pos),
            pitch: n.pitch,
            vel: n.vel,
            articulation: Articulation::Normal,
            pitch_curve: vec![],
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 鼻歌っぽい信号: 倍音入り + 軽いビブラート + 緩い立ち上がり/減衰 + 微小ノイズ
    fn hum(seq: &[(u8, f32)], sr: f32) -> SampleData {
        let mut frames = Vec::new();
        let mut phase = 0.0f32;
        let mut t_global = 0.0f32;
        for &(pitch, secs) in seq {
            let n = (secs * sr) as usize;
            if pitch == 0 {
                frames.extend(std::iter::repeat_n(0.0f32, n));
                t_global += secs;
                continue;
            }
            let f0 = 440.0 * (2.0f32).powf((pitch as f32 - 69.0) / 12.0);
            for i in 0..n {
                let t = i as f32 / sr;
                let vib = 1.0 + 0.012 * (t_global * 5.0 * std::f32::consts::TAU).sin();
                phase += f0 * vib / sr;
                let ph = phase * std::f32::consts::TAU;
                let s = ph.sin() + 0.5 * (2.0 * ph).sin() + 0.25 * (3.0 * ph).sin();
                let env = (t / 0.03).min(1.0) * (((secs - t) / 0.05).min(1.0));
                let noise = ((i * 7919) % 1000) as f32 / 1000.0 - 0.5;
                frames.push(0.3 * s * env + 0.002 * noise);
                t_global += 1.0 / sr;
            }
        }
        SampleData {
            frames,
            sample_rate: sr,
        }
    }

    #[test]
    fn transcribes_a_hummed_melody() {
        let seq = [(60u8, 0.4f32), (62, 0.4), (64, 0.4), (62, 0.4), (67, 0.6)];
        let data = hum(&seq, 48_000.0);
        let notes = transcribe_mono(&data, &TranscribeOptions::default());
        let pitches: Vec<u8> = notes.iter().map(|n| n.pitch).collect();
        assert_eq!(pitches, vec![60, 62, 64, 62, 67], "{notes:?}");
        // 開始時刻は ±40ms
        let expected = [0.0, 0.4, 0.8, 1.2, 1.6];
        for (n, e) in notes.iter().zip(expected) {
            assert!(
                (n.start_sec - e).abs() < 0.04,
                "start {} vs {e}",
                n.start_sec
            );
        }
        assert!(notes.iter().all(|n| n.vel >= 30 && n.vel <= 120));
    }

    #[test]
    fn silence_and_reattack_split_notes() {
        // 同じ音程を 2 回、間に無音
        let seq = [(64u8, 0.3f32), (0, 0.15), (64, 0.3)];
        let data = hum(&seq, 48_000.0);
        let notes = transcribe_mono(&data, &TranscribeOptions::default());
        assert_eq!(notes.len(), 2, "{notes:?}");
        assert!(notes[1].start_sec > 0.4);
    }

    #[test]
    fn low_voice_is_not_octave_confused() {
        // 低い男声域(G2 = 98Hz)でもオクターブ上に化けない
        let data = hum(&[(43u8, 0.6f32)], 44_100.0);
        let notes = transcribe_mono(&data, &TranscribeOptions::default());
        assert_eq!(notes.len(), 1, "{notes:?}");
        assert_eq!(notes[0].pitch, 43);
    }

    #[test]
    fn converts_to_ticks_with_quantize() {
        let tempo = TempoMap::default(); // 120bpm: 1 拍 = 0.5 秒 = 960 tick
        let notes = vec![
            TranscribedNote {
                start_sec: 0.02,
                end_sec: 0.48,
                pitch: 60,
                vel: 90,
            },
            TranscribedNote {
                start_sec: 0.51,
                end_sec: 0.99,
                pitch: 62,
                vel: 90,
            },
        ];
        let out = to_clip_notes(&notes, &tempo, Tick(3840), Tick(3840), 240);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].pos, Tick(0));
        assert_eq!(out[0].dur, Tick(960));
        assert_eq!(out[1].pos, Tick(960));
        // クオンタイズなしはそのまま
        let raw = to_clip_notes(&notes, &tempo, Tick(3840), Tick(3840), 0);
        assert_eq!(raw[0].pos, Tick(38));
    }
}
