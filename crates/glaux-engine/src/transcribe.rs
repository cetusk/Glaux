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
    /// 探索する基本周波数の範囲(Hz)。声は 70〜1000、口笛は 500〜2500 程度
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
            pitch_hi_hz: 2600.0,
            min_note_ms: 80.0,
            min_clarity: 0.35,
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
    /// 音量の立ち上がりで区切られた(同じ音程の弾き直し・歌い直し)
    pub reattack: bool,
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

/// 細かい音量包絡(2ms 刻み、5ms 窓、dB)。ノートの立ち上がり位置の補正に使う。
const ENV_HOP_SEC: f32 = 0.002;
fn fine_envelope(data: &SampleData) -> Vec<f32> {
    let sr = data.sample_rate;
    let hop = (ENV_HOP_SEC * sr).round().max(1.0) as usize;
    let win = (0.005 * sr).round().max(1.0) as usize;
    let x = &data.frames;
    let mut out = Vec::with_capacity(x.len() / hop + 1);
    let mut pos = 0usize;
    while pos < x.len() {
        let e = (pos + win).min(x.len());
        let rms = (x[pos..e].iter().map(|s| s * s).sum::<f32>() / (e - pos).max(1) as f32).sqrt();
        out.push(20.0 * rms.max(1e-9).log10());
        pos += hop;
    }
    out
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
    // 有声判定に音量条件を加える(最大から -45dB 以内、かつ -60dBFS 以上)
    let floor_db = (peak_db - 45.0).max(-60.0);
    for f in &mut frames {
        if f.rms_db < floor_db {
            f.midi = None;
        }
    }
    median_filter(&mut frames);

    let hop = HOP_SEC as f64;
    let min_frames = ((opts.min_note_ms / 1000.0) / HOP_SEC).round().max(1.0) as usize;
    const STABLE: usize = 4; // 半音が変わったと認める安定フレーム数(40ms)
    const GAP: usize = 6; // 無声がこれ以上続いたらノート終了(60ms。息の混じりは無視)
    const HYST: f32 = 0.7; // 現在の音程からこれ以上(半音)離れないと変化と見なさない

    let mut notes: Vec<TranscribedNote> = Vec::new();
    // (開始 frame, pitch, dB 列, 再アタックで始まったか)
    let mut cur: Option<(usize, u8, Vec<f32>, bool)> = None;
    let mut gap = 0usize;

    let finish = |cur: &mut Option<(usize, u8, Vec<f32>, bool)>,
                  end_frame: usize,
                  notes: &mut Vec<TranscribedNote>| {
        if let Some((start, pitch, dbs, reattack)) = cur.take() {
            if end_frame > start {
                let mean_db = dbs.iter().sum::<f32>() / dbs.len().max(1) as f32;
                // -40dB → 50、-10dB → 110 の線形マップ
                let vel = (50.0 + (mean_db + 40.0) * 2.0).clamp(30.0, 120.0) as u8;
                notes.push(TranscribedNote {
                    start_sec: start as f64 * hop,
                    end_sec: end_frame as f64 * hop,
                    pitch,
                    vel,
                    reattack,
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
                if gap == GAP {
                    finish(&mut cur, i + 1 - gap, &mut notes);
                }
            }
            Some(m) => {
                gap = 0;
                let p = rounded(m);
                match &mut cur {
                    None => cur = Some((i, p, vec![f.rms_db], false)),
                    Some((start, pitch, dbs, _)) => {
                        // 音量の立ち上がり(同じ音程の再アタック "タタタ")
                        // 直前がノート内の最大から 6dB 以上落ちた「谷」であることも条件にし、
                        // 1 音の中の音量の揺れで切らない
                        let note_max = dbs.iter().cloned().fold(f32::MIN, f32::max);
                        let onset = i >= 3
                            && frames[i - 3].midi.is_some()
                            && f.rms_db - frames[i - 3].rms_db > 8.0
                            && frames[i - 3].rms_db < note_max - 6.0
                            && i - *start >= min_frames;
                        // 半音の変化: 現在の音程から HYST 以上離れた値が STABLE フレーム続く
                        let far = |mm: f32| (mm - *pitch as f32).abs() > HYST && rounded(mm) == p;
                        let changed = p != *pitch
                            && far(m)
                            && (1..STABLE)
                                .all(|k| frames.get(i + k).and_then(|g| g.midi).is_some_and(far));
                        if onset || changed {
                            finish(&mut cur, i, &mut notes);
                            cur = Some((i, p, vec![f.rms_db], onset));
                        } else {
                            dbs.push(f.rms_db);
                        }
                    }
                }
            }
        }
    }
    let n = frames.len();
    finish(&mut cur, n.saturating_sub(gap.min(n)), &mut notes);

    assign_pitches(&mut notes, &frames);
    absorb_glides(&mut notes, &frames);
    assign_pitches(&mut notes, &frames);
    merge_fragments(&mut notes, opts.min_note_ms as f64 / 1000.0);
    refine_onsets(&mut notes, &fine_envelope(data));
    notes
}

/// ノート区間の有声フレームのピッチ(MIDI 小数)。
fn note_pitches(n: &TranscribedNote, frames: &[Frame]) -> Vec<f32> {
    let hop = HOP_SEC as f64;
    let s = (n.start_sec / hop).round() as usize;
    let e = ((n.end_sec / hop).round() as usize).min(frames.len());
    frames
        .get(s..e.max(s))
        .map_or(vec![], |fs| fs.iter().filter_map(|f| f.midi).collect())
}

fn median(v: &mut [f32]) -> Option<f32> {
    if v.is_empty() {
        return None;
    }
    v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    Some(v[v.len() / 2])
}

/// ノートの音程を「冒頭 30% を除いた区間の中央値」で決め直す。
/// 出だしのしゃくれ(下から滑り上がる)に引っ張られないようにする。
fn assign_pitches(notes: &mut [TranscribedNote], frames: &[Frame]) {
    for n in notes.iter_mut() {
        let v = note_pitches(n, frames);
        if v.is_empty() {
            continue;
        }
        let skip = (v.len() * 3 / 10).min(v.len() - 1);
        let mut tail = v[skip..].to_vec();
        if let Some(m) = median(&mut tail) {
            n.pitch = m.round().clamp(0.0, 127.0) as u8;
        }
    }
}

/// しゃくれ・ずり下げの吸収: 短く(<150ms)音程が定まらない(区間内の揺れ 0.8 半音超、
/// または 1 フレームも隣の音に近づかない)断片が、隙間なく隣のノートへ滑り込んで
/// いるなら、その断片は隣のノートの一部とみなして結合する。
/// 音程が安定した短い音(速いパッセージの 16 分音符)は残す。
fn absorb_glides(notes: &mut Vec<TranscribedNote>, frames: &[Frame]) {
    const SHORT: f64 = 0.15;
    const TOUCH: f64 = 0.04;
    let mut i = 0;
    while i < notes.len() {
        let n = &notes[i];
        let dur = n.end_sec - n.start_sec;
        let v = note_pitches(n, frames);
        // 両端のフレームは解析窓が隣の音にまたがるので、揺れは中央部分で測る
        let edge = v.len() / 4;
        let mid = &v[edge..v.len() - edge];
        let spread = mid.iter().cloned().fold(f32::MIN, f32::max)
            - mid.iter().cloned().fold(f32::MAX, f32::min);
        let unstable = mid.len() < 2 || spread > 0.8;
        if dur >= SHORT || !unstable {
            i += 1;
            continue;
        }
        // しゃくれ: 次の音へ滑り上がる(次が再アタックでない = 声が続いている)
        let into_next = notes.get(i + 1).is_some_and(|m| {
            !m.reattack
                && m.start_sec - n.end_sec < TOUCH
                && (m.pitch as i32 - n.pitch as i32).abs() <= 4
        });
        // ずり下げ: 前の音の終わりから続いている
        let from_prev = i > 0 && {
            let p = &notes[i - 1];
            !n.reattack
                && n.start_sec - p.end_sec < TOUCH
                && (p.pitch as i32 - n.pitch as i32).abs() <= 4
        };
        if into_next {
            let start = n.start_sec;
            let reattack = n.reattack;
            notes.remove(i);
            notes[i].start_sec = start;
            notes[i].reattack = reattack;
        } else if from_prev {
            let end = n.end_sec;
            notes.remove(i);
            notes[i - 1].end_sec = end;
        } else {
            i += 1;
        }
    }
}

/// 断片の後処理:
/// 1. 同じ音程の 2 音に挟まれた短い 1 半音違いの音(半音境界のふらつき)は両隣の音程に直す
///    (全音以上の刺繍音は本物のメロディとして残す)
/// 2. 隙間なく隣接する 1 半音差のごく短い音(<100ms、再アタックでない)は
///    長い方の隣の音程に直す(音の終わり・始まりの半音のちらつき)
/// 3. 同じ音程が短い隙間(120ms 未満)で続き、再アタックでなければ結合する
/// 4. それでも短すぎる(min_note_sec 未満)ノートは捨てる
fn merge_fragments(notes: &mut Vec<TranscribedNote>, min_note_sec: f64) {
    const MERGE_GAP: f64 = 0.12;
    const FLICKER: f64 = 0.10;
    for i in 0..notes.len() {
        let dur = notes[i].end_sec - notes[i].start_sec;
        if dur >= FLICKER || notes[i].reattack {
            continue;
        }
        let near = |j: usize| -> Option<(f64, u8)> {
            let m = notes.get(j)?;
            let (a, b) = if j < i {
                (m, &notes[i])
            } else {
                (&notes[i], m)
            };
            let touching = b.start_sec - a.end_sec < 0.04;
            let semitone = (m.pitch as i32 - notes[i].pitch as i32).abs() == 1;
            let longer = m.end_sec - m.start_sec > dur;
            (touching && semitone && longer).then_some((m.end_sec - m.start_sec, m.pitch))
        };
        let prev = i.checked_sub(1).and_then(near);
        let next = near(i + 1).filter(|_| !notes[i + 1].reattack);
        let pick = match (prev, next) {
            (Some(a), Some(b)) => Some(if a.0 >= b.0 { a.1 } else { b.1 }),
            (a, b) => a.or(b).map(|x| x.1),
        };
        if let Some(p) = pick {
            notes[i].pitch = p;
        }
    }
    let mut i = 1;
    while i + 1 < notes.len() {
        let (a, b, c) = (&notes[i - 1], &notes[i], &notes[i + 1]);
        let short = b.end_sec - b.start_sec < 0.15;
        if short
            && a.pitch == c.pitch
            && (b.pitch as i32 - a.pitch as i32).abs() == 1
            && b.start_sec - a.end_sec < MERGE_GAP
            && c.start_sec - b.end_sec < MERGE_GAP
        {
            notes[i].pitch = a.pitch;
        }
        i += 1;
    }
    let mut out: Vec<TranscribedNote> = Vec::with_capacity(notes.len());
    for n in notes.drain(..) {
        if let Some(prev) = out.last_mut() {
            if prev.pitch == n.pitch && !n.reattack && n.start_sec - prev.end_sec < MERGE_GAP {
                prev.end_sec = n.end_sec;
                prev.vel = prev.vel.max(n.vel);
                continue;
            }
        }
        out.push(n);
    }
    out.retain(|n| n.end_sec - n.start_sec >= min_note_sec);
    *notes = out;
}

/// ノートの開始を音量の立ち上がり点に合わせる。
/// ピッチ検出の解析窓は前方に広がるため、無音から始まるノートは実際の発声より
/// 早く(窓が声を含み始めた時点で)検出されることも、遅く確定することもある。
/// 細かい包絡で「ノート冒頭 80ms のピーク -15dB」を最初に超える点を
/// 開始の前後(-100ms〜+60ms)で探し、そこを開始にする。
fn refine_onsets(notes: &mut [TranscribedNote], env: &[f32]) {
    let hop = ENV_HOP_SEC as f64;
    for i in 0..notes.len() {
        let start = notes[i].start_sec;
        let prev_end = if i > 0 { notes[i - 1].end_sec } else { 0.0 };
        // 直前の音と地続き(音程変化で区切られた)なら動かさない
        if i > 0 && start - prev_end < 0.03 && !notes[i].reattack {
            continue;
        }
        let s_idx = (start / hop) as usize;
        let head_end = (((start + 0.08).min(notes[i].end_sec)) / hop) as usize;
        let Some(head) = env.get(s_idx..head_end.min(env.len())) else {
            continue;
        };
        if head.is_empty() {
            continue;
        }
        let peak = head.iter().cloned().fold(f32::MIN, f32::max);
        let target = peak - 15.0;
        let floor = ((start - 0.1).max(prev_end) / hop) as usize;
        let ceil = (((start + 0.06).min(notes[i].end_sec)) / hop) as usize;
        if let Some(k) = (floor..ceil.min(env.len())).find(|&k| env[k] >= target) {
            notes[i].start_sec = k as f64 * hop;
        }
    }
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
    fn breath_noise_does_not_fragment_a_note() {
        // 1 秒のロングトーンの途中に 30ms の無音(息の混じり)を 3 回入れても 1 音
        let mut data = hum(&[(65u8, 1.0f32)], 48_000.0);
        for &at in &[0.3f32, 0.55, 0.8] {
            let s = (at * 48_000.0) as usize;
            for v in &mut data.frames[s..s + 1440] {
                *v = 0.0;
            }
        }
        let notes = transcribe_mono(&data, &TranscribeOptions::default());
        assert_eq!(notes.len(), 1, "{notes:?}");
        assert!(notes[0].end_sec - notes[0].start_sec > 0.9);
    }

    #[test]
    fn semitone_wobble_stays_one_note() {
        // 62 の周りを ±55 セントでゆっくり揺れる(半音境界をまたぐ)→ 1 音
        let sr = 48_000.0f32;
        let n = (1.2 * sr) as usize;
        let mut phase = 0.0f32;
        let frames: Vec<f32> = (0..n)
            .map(|i| {
                let t = i as f32 / sr;
                let cents = 55.0 * (t * 2.0 * std::f32::consts::TAU).sin();
                let f0 = 293.66 * (2.0f32).powf(cents / 1200.0);
                phase += f0 / sr;
                let ph = phase * std::f32::consts::TAU;
                0.3 * (ph.sin() + 0.5 * (2.0 * ph).sin())
            })
            .collect();
        let data = SampleData {
            frames,
            sample_rate: sr,
        };
        let notes = transcribe_mono(&data, &TranscribeOptions::default());
        assert_eq!(notes.len(), 1, "{notes:?}");
        assert_eq!(notes[0].pitch, 62);
    }

    #[test]
    fn onset_is_refined_to_the_energy_rise() {
        // 0.5 秒の無音のあと発声。検出遅れが補正されて開始が ±15ms に入る
        let data = hum(&[(0u8, 0.5f32), (67, 0.5)], 48_000.0);
        let notes = transcribe_mono(&data, &TranscribeOptions::default());
        assert_eq!(notes.len(), 1, "{notes:?}");
        assert!(
            (notes[0].start_sec - 0.5).abs() < 0.015,
            "start={}",
            notes[0].start_sec
        );
    }

    /// 各ノートの出だしを `scoop_cents` 下から `scoop_sec` かけて滑り上げる鼻歌
    fn hum_scooped(seq: &[(u8, f32)], scoop_cents: f32, scoop_sec: f32, sr: f32) -> SampleData {
        let mut frames = Vec::new();
        let mut phase = 0.0f32;
        for &(pitch, secs) in seq {
            let n = (secs * sr) as usize;
            let f0 = 440.0 * (2.0f32).powf((pitch as f32 - 69.0) / 12.0);
            for i in 0..n {
                let t = i as f32 / sr;
                let c = -scoop_cents * (1.0 - (t / scoop_sec).min(1.0));
                phase += f0 * (2.0f32).powf(c / 1200.0) / sr;
                let ph = phase * std::f32::consts::TAU;
                let env = (t / 0.02).min(1.0) * (((secs - t) / 0.03).min(1.0));
                frames.push(0.3 * env * (ph.sin() + 0.4 * (2.0 * ph).sin()));
            }
        }
        SampleData {
            frames,
            sample_rate: sr,
        }
    }

    #[test]
    fn scooped_onsets_are_absorbed() {
        // 各音の頭で 3 半音下から 90ms かけてしゃくり上げる
        let seq = [(60u8, 0.45f32), (64, 0.45), (67, 0.45), (64, 0.6)];
        let data = hum_scooped(&seq, 300.0, 0.09, 48_000.0);
        let notes = transcribe_mono(&data, &TranscribeOptions::default());
        let pitches: Vec<u8> = notes.iter().map(|n| n.pitch).collect();
        assert_eq!(pitches, vec![60, 64, 67, 64], "{notes:?}");
        for (n, e) in notes.iter().zip([0.0, 0.45, 0.9, 1.35]) {
            assert!(
                (n.start_sec - e).abs() < 0.05,
                "start {} vs {e}",
                n.start_sec
            );
        }
    }

    #[test]
    fn fast_sixteenths_are_kept() {
        // 120bpm の 16 分(125ms)の音階。しゃくれ吸収で潰さない
        let seq: Vec<(u8, f32)> = [60u8, 62, 64, 65, 67, 65, 64, 62]
            .iter()
            .map(|&p| (p, 0.125f32))
            .collect();
        let data = hum(&seq, 48_000.0);
        let notes = transcribe_mono(&data, &TranscribeOptions::default());
        let pitches: Vec<u8> = notes.iter().map(|n| n.pitch).collect();
        assert_eq!(pitches, vec![60, 62, 64, 65, 67, 65, 64, 62], "{notes:?}");
    }

    #[test]
    fn whistle_range_is_detected() {
        // 口笛(ほぼ純音、1500Hz 前後 = MIDI 90 付近)
        let sr = 48_000.0f32;
        let mut frames = Vec::new();
        let mut phase = 0.0f32;
        for &(pitch, secs) in &[(88u8, 0.4f32), (91, 0.4), (93, 0.5)] {
            let f0 = 440.0 * (2.0f32).powf((pitch as f32 - 69.0) / 12.0);
            for i in 0..(secs * sr) as usize {
                let t = i as f32 / sr;
                phase += f0 / sr;
                let env = (t / 0.02).min(1.0) * (((secs - t) / 0.03).min(1.0));
                frames.push(0.3 * env * (phase * std::f32::consts::TAU).sin());
            }
        }
        let notes = transcribe_mono(
            &SampleData {
                frames,
                sample_rate: sr,
            },
            &TranscribeOptions::default(),
        );
        let pitches: Vec<u8> = notes.iter().map(|n| n.pitch).collect();
        assert_eq!(pitches, vec![88, 91, 93], "{notes:?}");
    }

    #[test]
    fn semitone_flicker_at_note_end_is_absorbed() {
        // 64 のロングトーンの最後 60ms だけ 65 に上ずる → 1 音 64
        let data = hum(&[(64u8, 0.5f32), (65, 0.06)], 48_000.0);
        let notes = transcribe_mono(&data, &TranscribeOptions::default());
        let pitches: Vec<u8> = notes.iter().map(|n| n.pitch).collect();
        assert_eq!(pitches, vec![64], "{notes:?}");
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
                reattack: false,
            },
            TranscribedNote {
                start_sec: 0.51,
                end_sec: 0.99,
                pitch: 62,
                vel: 90,
                reattack: false,
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
