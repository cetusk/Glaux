//! 曲から起こす: 曲の骨組み(拍・小節頭・テンポ・拍子・チューニング・調・和音)の解析。
//!
//! 参考曲の音声から、作業場の最初の手順「格子を合わせる」と、AI が参考曲を読むための材料を作る。
//! 方式は段ごとに選べるようにしておき、評価(`examples/recon_eval.rs`)で比べて既定を決める。

use glaux_engine::mir_eval::ChordSpan;
use serde::Serialize;

/// 拍・小節頭の求め方
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum BeatMethod {
    /// Beat This! の公式の「最小の後処理」(ロジットの極大)
    Minimal,
    /// 動的ベイジアンネットワーク(madmom の移植。拍子と小節頭を小節単位で整合させる)
    #[default]
    Dbn,
}

/// 和音の求め方
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ChordMethod {
    /// 小節ごとのクロマをテンプレートに当てる(構成の解析と同じ。三和音だけ)
    PerBar,
    /// 拍ごとの NNLS クロマ(チューニング補正つき)を和音の形に当て、隠れマルコフモデルでつなぐ
    #[default]
    Nnls,
}

/// 解析の方式
#[derive(Clone, Debug, Default)]
pub struct SkeletonOptions {
    pub beat: BeatMethod,
    pub chord: ChordMethod,
    pub chord_opts: glaux_engine::tonal::ChordOptions,
}

/// 調の推定
#[derive(Clone, Debug, Serialize)]
pub struct KeyGuess {
    /// 主音のピッチクラス(C = 0)
    pub tonic: u8,
    pub minor: bool,
    /// "C major" / "A minor"
    pub name: String,
    pub confidence: f64,
}

/// テンポの区間(この拍から次の区間の頭まで一定)
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct TempoSegment {
    /// 何番目の拍から(0 始まり。`Skeleton::beats` の番号)
    pub beat: usize,
    /// その拍の時刻(秒)
    pub time: f64,
    pub bpm: f64,
}

/// 曲の骨組み
#[derive(Clone, Debug, Serialize)]
pub struct Skeleton {
    pub duration_sec: f64,
    /// ビートの時刻(秒)
    pub beats: Vec<f64>,
    /// 小節頭の時刻(秒)
    pub downbeats: Vec<f64>,
    /// 代表のテンポ(拍の間隔の中央値から)
    pub bpm: Option<f64>,
    /// テンポの変化(最初の拍から。区間の中は一定)。曲の格子にそのまま使える
    pub tempo_map: Vec<TempoSegment>,
    /// 1 小節のビート数
    pub beats_per_bar: Option<u32>,
    /// チューニングのずれ(セント。A = 440Hz からの。-50〜+50)
    pub tuning_cents: f64,
    pub key: Option<KeyGuess>,
    /// 和音の並び(秒。"N" は和音なし)
    pub chords: Vec<ChordSpan>,
    /// ノリ(ハネ具合・揺れ・細かさ・密度)
    pub groove: Option<glaux_engine::feel::Groove>,
}

/// 曲の骨組みを解析する(モノラル音声)
pub fn analyze(
    frames: &[f32],
    sample_rate: f32,
    opts: SkeletonOptions,
) -> Result<Skeleton, String> {
    let dur = frames.len() as f64 / sample_rate as f64;
    let t = match opts.beat {
        BeatMethod::Dbn => glaux_ml::beats::track_dbn(frames, sample_rate, &Default::default()),
        BeatMethod::Minimal => glaux_ml::beats::track(frames, sample_rate),
    }
    .map_err(|e| e.to_string())?;
    let bpb = t.beats_per_bar();
    let mut beats: Vec<f64> = t.beats.iter().map(|&b| b as f64).collect();
    let mut downbeats: Vec<f64> = t.downbeats.iter().map(|&b| b as f64).collect();
    let onsets = glaux_engine::feel::onsets(frames, sample_rate);
    let mut groove = glaux_engine::feel::analyze(&onsets, &beats);
    // 8 分の裏が拍の半分より前(逆向きのハネ)なら、拍がハネた裏に乗っている。実際の曲ではまず無いので、
    // 拍をその「裏」の位置(本当の拍の頭)へずらす
    if let Some(p) = groove
        .as_ref()
        .and_then(|g| g.swing_8th_pct)
        .filter(|p| *p < 45.0)
    {
        let shift = |v: &[f64]| -> Vec<f64> {
            v.iter()
                .map(|&x| {
                    let i = beats.partition_point(|b| *b <= x).saturating_sub(1);
                    let ibi = if i + 1 < beats.len() {
                        beats[i + 1] - beats[i]
                    } else if i > 0 {
                        beats[i] - beats[i - 1]
                    } else {
                        0.5
                    };
                    x + ibi * p / 100.0
                })
                .collect()
        };
        downbeats = shift(&downbeats);
        beats = shift(&beats);
        groove = glaux_engine::feel::analyze(&onsets, &beats);
    }
    // 構成の解析と同じく、小節頭が取れなければビート n 個ごと
    let bars: Vec<f64> = if downbeats.len() >= 4 {
        downbeats.clone()
    } else if beats.len() >= 16 {
        beats
            .iter()
            .step_by(bpb.unwrap_or(4).max(1) as usize)
            .copied()
            .collect()
    } else {
        (0..(dur / 2.0) as usize).map(|i| i as f64 * 2.0).collect()
    };
    let tempo_map = tempo_map(&beats);
    let bpm = median_bpm(&beats);
    let tuning = glaux_engine::tonal::estimate_tuning(frames, sample_rate);
    let (key, chords) = match opts.chord {
        ChordMethod::PerBar => match glaux_engine::structure::analyze(frames, sample_rate, &bars) {
            Ok(st) => {
                let key = st.key.as_ref().map(|k| KeyGuess {
                    tonic: k.tonic,
                    minor: k.mode == "minor",
                    name: k.name.clone(),
                    confidence: k.confidence,
                });
                let chords = st
                    .bar_bounds
                    .iter()
                    .zip(&st.bar_chords)
                    .map(|(&(start, end), c)| ChordSpan {
                        start,
                        end,
                        symbol: if c == "N.C." {
                            "N".to_owned()
                        } else {
                            c.clone()
                        },
                    })
                    .collect();
                (key, chords)
            }
            Err(_) => (None, Vec::new()),
        },
        ChordMethod::Nnls => {
            let (bounds, pos) = beat_segments(&beats, &downbeats, dur);
            let notes = glaux_engine::tonal::note_activations(frames, sample_rate, tuning, &bounds);
            let chords = glaux_engine::tonal::recognize_chords(
                &notes,
                &bounds,
                &pos,
                bpb.unwrap_or(4),
                &opts.chord_opts,
            );
            let key = glaux_engine::tonal::estimate_key(&notes, &bounds).map(
                |(tonic, minor, confidence)| KeyGuess {
                    tonic,
                    minor,
                    name: format!(
                        "{} {}",
                        glaux_core::chord::note_name(tonic + 60)
                            .trim_end_matches(|c: char| c.is_ascii_digit()),
                        if minor { "minor" } else { "major" }
                    ),
                    confidence,
                },
            );
            (key, chords)
        }
    };
    Ok(Skeleton {
        duration_sec: dur,
        beats,
        downbeats,
        bpm,
        tempo_map,
        beats_per_bar: bpb,
        tuning_cents: (tuning * 10.0).round() / 10.0,
        key,
        chords,
        groove,
    })
}

// ---- パートの譜起こし(段階 2) ----------------------------------------------

/// パートの種類(譜起こしの方式の既定を決める)
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PartKind {
    /// 歌・旋律(単音)
    Melody,
    /// ベース(単音・低い)
    Bass,
    /// ピアノ・ギター・その他(和音)
    Poly,
    /// ドラム(キック・スネア・ハイハット・シンバル。GM のドラムの番号で)
    Drums,
}

/// 譜起こしの方式
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NoteMethod {
    /// 単音: basic-pitch(単音にまとめる)を基本に、SwiftF0 の方が鳴っている所をはっきり多く覆うときは SwiftF0
    Auto,
    /// 単音: SwiftF0 の音程の列を音符に区切る
    SwiftF0,
    /// 単音: YIN(鼻歌の譜起こしと同じ)
    Yin,
    /// 和音: basic-pitch
    BasicPitch,
    /// ドラム: 打楽器の成分を取り出し、帯域ごとの立ち上がりでキック・スネア・ハイハット・シンバルに分ける(分けたドラムの音から)
    Drums,
    /// ドラム: 分けていない曲そのものから(弱い立ち上がりを候補にしない設定で)
    DrumsMix,
}

impl PartKind {
    pub fn default_method(self) -> NoteMethod {
        match self {
            PartKind::Melody | PartKind::Bass => NoteMethod::Auto,
            PartKind::Poly => NoteMethod::BasicPitch,
            PartKind::Drums => NoteMethod::Drums,
        }
    }

    /// 音域(MIDI)
    fn range(self) -> (f32, f32) {
        match self {
            PartKind::Melody => (40.0, 96.0),
            PartKind::Bass => (24.0, 64.0),
            PartKind::Poly => (21.0, 108.0),
            PartKind::Drums => (35.0, 81.0),
        }
    }
}

/// 起こした音符(秒)
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct RawNote {
    pub start: f64,
    pub end: f64,
    pub pitch: u8,
    pub vel: u8,
}

/// チューニングのずれを打ち消すように音声の高さを変える(長さも少し変わる)。(音声, 新しい時刻 → 元の時刻の倍率)
fn untune(frames: &[f32], sr: f32, cents: f64) -> (Vec<f32>, f64) {
    if cents.abs() < 3.0 {
        return (frames.to_vec(), 1.0);
    }
    // 高さを r 倍にする: 元の 1 サンプルを 1/r サンプルに伸ばす(r < 1 で低く)
    let r = 2f64.powf(-cents / 1200.0);
    let out = glaux_ml::resample(frames, sr, (sr as f64 / r) as f32);
    (out, r)
}

/// パートの音声(分けた音か、ミックス)から音符を起こす。`tuning_cents` は曲のチューニングのずれ(骨組みの解析から)
pub fn transcribe_part(
    frames: &[f32],
    sr: f32,
    kind: PartKind,
    method: NoteMethod,
    tuning_cents: f64,
) -> Result<Vec<RawNote>, String> {
    let (audio, scale) = untune(frames, sr, tuning_cents);
    let (lo, hi) = kind.range();
    let onsets = || -> Vec<f64> {
        glaux_engine::feel::onsets(&audio, sr)
            .iter()
            .filter(|o| o.strength >= 0.15)
            .map(|o| o.time)
            .collect()
    };
    let swift = || -> Result<Vec<RawNote>, String> {
        let pitch = glaux_ml::pitch::track(&audio, sr).map_err(|e| e.to_string())?;
        let on = onsets();
        let mut v = mono_notes(&pitch, &on, &audio, sr, lo, hi);
        snap_to_onsets(&mut v, &on);
        Ok(v)
    };
    let basic = || -> Result<Vec<RawNote>, String> {
        let opts = glaux_ml::PolyOptions {
            min_pitch: lo as u8,
            max_pitch: hi as u8,
            ..Default::default()
        };
        Ok(glaux_ml::transcribe_poly(&audio, sr, &opts)
            .map_err(|e| e.to_string())?
            .into_iter()
            .map(|n| RawNote {
                start: n.start_sec,
                end: n.end_sec,
                pitch: n.pitch,
                vel: (40.0 + n.amplitude.clamp(0.0, 1.0) * 87.0) as u8,
            })
            .collect())
    };
    let refine = |v: Vec<RawNote>| refine_note_ends(v, &audio, sr);
    let mono = matches!(kind, PartKind::Melody | PartKind::Bass);
    let mut notes = match method {
        NoteMethod::Auto => {
            let a = to_mono(basic()?, kind == PartKind::Bass);
            let b = swift()?;
            // 鳴っている所(音量が最大から 35dB 以内)をどれだけ音符が覆うか。basic-pitch を基本にし、
            // SwiftF0 の方がはっきり多く覆うときだけ替える(basic-pitch が音を拾えない音色)
            // (終わりを決め直す前の音符で比べる。決め直すと短くなり、覆う割合が下がるため)
            let (ca, cb) = (coverage(&a, &audio, sr), coverage(&b, &audio, sr));
            if cb > ca + 0.1 {
                b
            } else {
                refine(a)
            }
        }
        NoteMethod::Drums | NoteMethod::DrumsMix => {
            // 打楽器の成分(分けたドラムの音でも、ほかの楽器の持続音の残りを除ける)
            let perc = glaux_engine::separate::hpss(&audio).percussive;
            let opts = if method == NoteMethod::DrumsMix {
                glaux_engine::drum_transcribe::DrumOptions::mixed()
            } else {
                Default::default()
            };
            glaux_engine::drum_transcribe::transcribe(&perc, sr, &opts)
                .into_iter()
                .map(|h| RawNote {
                    start: h.time,
                    end: h.time + 0.1,
                    pitch: h.kind.gm_note(),
                    vel: h.vel,
                })
                .collect()
        }
        NoteMethod::SwiftF0 => swift()?,
        NoteMethod::Yin => {
            let data = glaux_dsp::SampleData::mono(audio.clone(), sr);
            let opts = glaux_engine::transcribe::TranscribeOptions {
                pitch_lo_hz: 440.0 * 2f32.powf((lo - 69.0) / 12.0),
                pitch_hi_hz: 440.0 * 2f32.powf((hi - 69.0) / 12.0),
                ..Default::default()
            };
            glaux_engine::transcribe::transcribe_mono(&data, &opts)
                .into_iter()
                .map(|n| RawNote {
                    start: n.start_sec,
                    end: n.end_sec,
                    pitch: n.pitch,
                    vel: n.vel,
                })
                .collect()
        }
        NoteMethod::BasicPitch => {
            let v = refine(basic()?);
            if mono {
                to_mono(v, kind == PartKind::Bass)
            } else {
                v
            }
        }
    };
    for n in notes.iter_mut() {
        n.start *= scale;
        n.end *= scale;
    }
    Ok(notes)
}

/// 分けていない曲(ミックス)から、複数のパートをまとめて起こす。basic-pitch で 1 回起こし、声部に分ける:
/// 旋律 = 重なっている音のうちいちばん上(G3 以上)、ベース = いちばん下(C4 以下)、和音 = 旋律・ベースに取られなかった残り。
/// `kinds` の順に音符の並びを返す(同じ種類が 2 つあれば、残りを最初の 1 つに入れ、ほかは空)
pub fn transcribe_mix_parts(
    mix: &[f32],
    sr: f32,
    kinds: &[PartKind],
    tuning_cents: f64,
) -> Result<Vec<Vec<RawNote>>, String> {
    let all = transcribe_part(
        mix,
        sr,
        PartKind::Poly,
        NoteMethod::BasicPitch,
        tuning_cents,
    )?;
    let overlaps = |a: &RawNote, b: &RawNote| -> bool {
        let ov = a.end.min(b.end) - a.start.max(b.start);
        ov > 0.5 * (a.end - a.start).min(0.25)
    };
    let want_melody = kinds.contains(&PartKind::Melody);
    let want_bass = kinds.contains(&PartKind::Bass);
    let mut role = vec![0u8; all.len()]; // 0 = 和音, 1 = 旋律, 2 = ベース
    for (i, n) in all.iter().enumerate() {
        let higher = all.iter().any(|m| m.pitch > n.pitch && overlaps(n, m));
        let lower = all.iter().any(|m| m.pitch < n.pitch && overlaps(n, m));
        if want_bass && !lower && n.pitch <= 60 {
            role[i] = 2;
        } else if want_melody && !higher && n.pitch >= 55 {
            role[i] = 1;
        }
    }
    let pick = |r: u8| -> Vec<RawNote> {
        all.iter()
            .zip(&role)
            .filter(|(_, x)| **x == r)
            .map(|(n, _)| *n)
            .collect()
    };
    let mut used = [false; 3];
    Ok(kinds
        .iter()
        .map(|k| {
            let r = match k {
                PartKind::Poly => 0,
                PartKind::Melody => 1,
                PartKind::Bass => 2,
                PartKind::Drums => return Vec::new(),
            };
            if std::mem::replace(&mut used[r as usize], true) {
                return Vec::new();
            }
            let v = pick(r);
            match k {
                PartKind::Poly | PartKind::Drums => v,
                PartKind::Melody => to_mono(v, false),
                PartKind::Bass => to_mono(v, true),
            }
        })
        .collect())
}

/// 和音で起こした音符を単音にまとめる: 立ち上がりが 50ms 以内に重なった音は、ベースなら一番低い音、
/// 旋律なら一番強い音だけを残す(倍音を別の音と取り違えた分を捨てる)。前の音は次の音の頭で切る
fn to_mono(mut notes: Vec<RawNote>, lowest: bool) -> Vec<RawNote> {
    notes.sort_by(|a, b| a.start.total_cmp(&b.start));
    let mut out: Vec<RawNote> = Vec::new();
    let mut i = 0;
    while i < notes.len() {
        let mut j = i + 1;
        while j < notes.len() && notes[j].start - notes[i].start <= 0.05 {
            j += 1;
        }
        let pick = notes[i..j]
            .iter()
            .copied()
            .min_by(|a, b| {
                if lowest {
                    a.pitch.cmp(&b.pitch)
                } else {
                    b.vel.cmp(&a.vel)
                }
            })
            .unwrap_or(notes[i]);
        // 長く伸びた前の音の中で鳴った、前の音の倍音(オクターブ・5 度上)は捨てる
        if let Some(prev) = out.last() {
            let iv = pick.pitch as i32 - prev.pitch as i32;
            if pick.start < prev.end - 0.05 && (iv == 12 || iv == 19 || iv == 24) {
                i = j;
                continue;
            }
        }
        if let Some(prev) = out.last_mut() {
            if prev.end > pick.start {
                prev.end = pick.start;
            }
        }
        if pick.end - pick.start >= 0.04 {
            out.push(pick);
        }
        i = j;
    }
    out
}

/// 音符の頭を、すぐ前(120ms 以内)の立ち上がりへ寄せる(音程が落ち着くまでの遅れを取り戻す)
fn snap_to_onsets(notes: &mut [RawNote], onsets: &[f64]) {
    let mut prev_end = f64::NEG_INFINITY;
    for n in notes.iter_mut() {
        let best = onsets
            .iter()
            .copied()
            .filter(|&o| o <= n.start + 0.02 && o >= n.start - 0.12 && o >= prev_end - 0.01)
            .max_by(|a, b| a.total_cmp(b));
        if let Some(o) = best {
            n.start = o.min(n.start);
        }
        prev_end = n.end;
    }
}

/// basic-pitch の音符の終わりを、音の高さごとの大きさの動きから決め直す: 立ち上がりの直後の最大から
/// END_DROP_DB 下がった所で終える(短くするだけ)。basic-pitch は残響・余韻・ディレイの響きで音符を長く伸ばし、
/// 短く切った音(スタブ・アルペジオ・ベース)の終わりが合わない
fn refine_note_ends(mut notes: Vec<RawNote>, audio: &[f32], sr: f32) -> Vec<RawNote> {
    let pe = glaux_engine::tonal::pitch_energy(audio, sr, 0.0);
    if pe.db.is_empty() {
        return notes;
    }
    for n in notes.iter_mut() {
        let peak = pe.max(n.pitch, n.start, n.start + 0.06);
        let (i0, i1) = (pe.frame(n.start + 0.03), pe.frame(n.end));
        if let Some(k) = (i0..i1).find(|&k| pe.db[k][n.pitch as usize] < peak - END_DROP_DB) {
            n.end = (k as f64 * pe.hop).max(n.start + 0.03);
        }
    }
    notes
}

/// 音符を終えるとみなす、立ち上がりの直後の最大からの下がり(dB)
const END_DROP_DB: f32 = 12.0;

/// 音が鳴っている所(20ms ごとの音量が最大から 35dB 以内)のうち、どれかの音符が覆っている割合
fn coverage(notes: &[RawNote], audio: &[f32], sr: f32) -> f64 {
    let hop = (0.02 * sr) as usize;
    if hop == 0 || audio.len() < hop {
        return 0.0;
    }
    let db: Vec<f32> = audio
        .chunks(hop)
        .map(|c| {
            10.0 * (c.iter().map(|x| x * x).sum::<f32>() / c.len() as f32)
                .max(1e-12)
                .log10()
        })
        .collect();
    let max = db.iter().cloned().fold(f32::MIN, f32::max);
    let (mut active, mut covered) = (0usize, 0usize);
    for (i, d) in db.iter().enumerate() {
        if *d < max - 35.0 {
            continue;
        }
        active += 1;
        let t = (i as f64 + 0.5) * 0.02;
        if notes.iter().any(|n| n.start <= t && t < n.end) {
            covered += 1;
        }
    }
    if active == 0 {
        0.0
    } else {
        covered as f64 / active as f64
    }
}

/// SwiftF0 の音程の列(16ms ごと)を音符に区切る。
/// 有声のフレームの音程(MIDI、前後 5 フレームの中央値)が、今の音符の音程から 0.7 半音より離れて 2 フレーム続けば次の音符、
/// 2 フレーム以上の無声で音符を閉じる。同じ音程のまま立ち上がり(`onsets`)があれば弾き直しとして分ける。
/// 音程は区間の中央値を四捨五入、強さは立ち上がりの後 50ms の音量から
fn mono_notes(
    pitch: &[glaux_ml::pitch::PitchEstimate],
    onsets: &[f64],
    audio: &[f32],
    sr: f32,
    lo: f32,
    hi: f32,
) -> Vec<RawNote> {
    const HOP: f64 = 0.016;
    const MIN_LEN: f64 = 0.05;
    let raw: Vec<Option<f32>> = pitch
        .iter()
        .map(|p| {
            (p.confidence >= 0.5 && p.f0 > 0.0)
                .then(|| 69.0 + 12.0 * (p.f0 / 440.0).log2())
                .filter(|m| (lo..=hi).contains(m))
        })
        .collect();
    // 有声のフレームだけで中央値(オクターブの飛びを抑える)
    let smooth: Vec<Option<f32>> = (0..raw.len())
        .map(|i| {
            raw[i]?;
            let mut w: Vec<f32> = raw[i.saturating_sub(2)..(i + 3).min(raw.len())]
                .iter()
                .flatten()
                .copied()
                .collect();
            w.sort_by(f32::total_cmp);
            Some(w[w.len() / 2])
        })
        .collect();
    let time = |i: usize| -> f64 { pitch.get(i).map_or(i as f64 * HOP, |p| p.time as f64) };
    // 区切り
    let mut spans: Vec<(usize, usize)> = Vec::new(); // [開始, 終了) フレーム
    let mut cur: Option<(usize, Vec<f32>)> = None;
    let mut gap = 0;
    let mut away = 0;
    for (i, m) in smooth.iter().enumerate() {
        match (m, cur.as_mut()) {
            (None, Some(_)) => {
                gap += 1;
                if gap >= 2 {
                    if let Some((s, _)) = cur.take() {
                        spans.push((s, i + 1 - gap));
                    }
                    gap = 0;
                }
            }
            (None, None) => {}
            (Some(m), None) => {
                cur = Some((i, vec![*m]));
                gap = 0;
                away = 0;
            }
            (Some(m), Some((s, ps))) => {
                gap = 0;
                let mut sorted = ps.clone();
                sorted.sort_by(f32::total_cmp);
                let med = sorted[sorted.len() / 2];
                if (m - med).abs() > 0.7 {
                    away += 1;
                    if away >= 2 {
                        let start = *s;
                        spans.push((start, i - 1));
                        cur = Some((i - 1, vec![*m]));
                        away = 0;
                        continue;
                    }
                } else {
                    away = 0;
                }
                ps.push(*m);
            }
        }
    }
    if let Some((s, _)) = cur {
        spans.push((s, smooth.len()));
    }
    // 区間 [t, t + 30ms) の音量(dB)
    let level = |t: f64| -> f32 {
        let a = ((t.max(0.0) * sr as f64) as usize).min(audio.len());
        let b = (a + (0.03 * sr) as usize).min(audio.len());
        if b <= a {
            return -90.0;
        }
        let e = audio[a..b].iter().map(|x| x * x).sum::<f32>() / (b - a) as f32;
        10.0 * e.max(1e-12).log10()
    };
    // 立ち上がりで弾き直しを分ける。音符の終わりの近く(約 100ms)の立ち上がりは次の音符のものなので分けない。
    // 分けた後ろが前の頭より 6dB 以上小さい・立ち上がりの所で音量が下がっていくなら、弾き直しではなく音を離したときの
    // 音色の変化なので分けない
    // (開始, 終了, 頭が立ち上がりか)
    let mut split: Vec<(usize, usize, bool)> = Vec::new();
    for (s, mut e) in spans {
        let mut start = s;
        let mut attack = true;
        let mut head = level(time(s));
        for &o in onsets {
            let k = ((o / HOP).round() as usize).min(e);
            if k > start + 4 && k + 6 < e {
                let l = level(time(k));
                // 立ち上がりの所で音量が下がっていく = 音を離した所。残りが短ければ余韻なので、音符をそこで終える
                if level(o + 0.01) < level(o - 0.04) - 1.8 {
                    if e - k <= 16 {
                        e = k;
                        break;
                    }
                    continue;
                }
                // 頭より大きく小さい(余韻の中の揺れ)なら分けない
                if l < head - 6.0 {
                    continue;
                }
                split.push((start, k, attack));
                start = k;
                attack = true;
                head = l;
            }
        }
        split.push((start, e, attack));
    }
    // 音程の揺れで分かれた、すき間なく続く同じ音程の区間は 1 つにする(頭が立ち上がりなら弾き直しとして残す)
    let med = |a: usize, b: usize| -> Option<u8> {
        let mut ps: Vec<f32> = smooth[a..b].iter().flatten().copied().collect();
        if ps.is_empty() {
            return None;
        }
        ps.sort_by(f32::total_cmp);
        Some(ps[ps.len() / 2].round().clamp(0.0, 127.0) as u8)
    };
    let onset_at = |k: usize| {
        onsets
            .iter()
            .any(|&o| ((o / HOP).round() as i64 - k as i64).abs() <= 2)
    };
    let mut merged: Vec<(usize, usize, bool)> = Vec::new();
    for seg in split {
        if let Some(last) = merged.last_mut() {
            if last.1 + 1 >= seg.0 && !onset_at(seg.0) && med(last.0, last.1) == med(seg.0, seg.1) {
                last.1 = seg.1;
                continue;
            }
        }
        merged.push(seg);
    }
    let split: Vec<(usize, usize)> = merged.into_iter().map(|(a, b, _)| (a, b)).collect();
    let rms_db = |t: f64| -> f32 {
        let a = ((t * sr as f64) as usize).min(audio.len());
        let b = (a + (0.05 * sr) as usize).min(audio.len());
        if b <= a {
            return -90.0;
        }
        let e = audio[a..b].iter().map(|x| x * x).sum::<f32>() / (b - a) as f32;
        10.0 * e.max(1e-12).log10()
    };
    split
        .into_iter()
        .filter_map(|(s, e)| {
            let mut ps: Vec<f32> = smooth[s..e].iter().flatten().copied().collect();
            if ps.is_empty() {
                return None;
            }
            ps.sort_by(f32::total_cmp);
            let pitch = ps[ps.len() / 2].round().clamp(0.0, 127.0) as u8;
            let (start, end) = (time(s), time(e.saturating_sub(1)) + HOP);
            (end - start >= MIN_LEN).then(|| RawNote {
                start,
                end,
                pitch,
                vel: ((rms_db(start) + 45.0) * 2.2 + 40.0).clamp(20.0, 127.0) as u8,
            })
        })
        .collect()
}

// ---- 格子を曲に写す(段階 2) ---------------------------------------------------

/// 格子を合わせるときの直し(作業場の「格子」の手順のボタン)
#[derive(Clone, Copy, Debug, serde::Deserialize, Serialize)]
pub struct GridOptions {
    /// 小節頭を何拍ずらすか(正 = 後ろへ)
    #[serde(default)]
    pub downbeat_shift: i32,
    /// 拍の取り方: 1 = そのまま、2 = 倍の速さ(拍の間に拍を足す)、0.5 = 半分(1 つおき)
    #[serde(default = "one")]
    pub tempo_factor: f64,
    /// 1 小節の拍数(省略で推定のまま)
    #[serde(default)]
    pub beats_per_bar: Option<u32>,
    /// 弱起なし: 最初の小節頭を 1 小節目にし、その前の音は切る。None なら、前が静か(無音・ノイズ)なら切る
    #[serde(default)]
    pub no_pickup: Option<bool>,
    /// 弱起の小節より前の音(前奏など)を残す(残すなら、その分の小節を前に足す)。None なら切る。
    /// 作業場では、前に音があるかで決める([`resolve_grid`])
    #[serde(default)]
    pub keep_intro: Option<bool>,
}

fn one() -> f64 {
    1.0
}

impl Default for GridOptions {
    fn default() -> Self {
        GridOptions {
            downbeat_shift: 0,
            tempo_factor: 1.0,
            beats_per_bar: None,
            no_pickup: None,
            keep_intro: None,
        }
    }
}

/// 曲に写す格子
#[derive(Clone, Debug, Serialize)]
pub struct GridPlan {
    /// 直した後の拍(音声の秒)
    pub beats: Vec<f64>,
    /// 1 小節の拍数
    pub beats_per_bar: u32,
    /// 最初の拍の前に置く拍の数(弱起・前奏を小節に収める分)。負なら、最初の小節頭の前の拍をその数だけ切った(弱起なし)
    pub lead_beats: i64,
    /// 音声の頭(0 秒)が曲のどの時刻に来るか(秒)。負なら音声の頭を切る
    pub audio_start_sec: f64,
    pub tempo: Vec<glaux_core::time::TempoEvent>,
}

impl GridPlan {
    pub fn tempo_map(&self) -> glaux_core::TempoMap {
        glaux_core::TempoMap::new(self.tempo.clone()).unwrap_or_default()
    }

    /// 音声の時刻(秒)→ 曲の tick
    pub fn tick_of(&self, map: &glaux_core::TempoMap, audio_sec: f64) -> f64 {
        map.seconds_to_tick_f64(self.audio_start_sec + audio_sec)
    }
}

/// 骨組みの拍から、曲のテンポ・拍子の格子を作る。最初の小節頭が小節の頭に来て、音声の頭が曲の 0 秒以降に入るように、
/// 前に拍を足す。拍が 8 つに満たなければ None
pub fn grid_plan(sk: &Skeleton, opts: &GridOptions) -> Option<GridPlan> {
    let ppq = glaux_core::time::PPQ;
    // 拍の取り方
    let mut beats = sk.beats.clone();
    let first_down = sk
        .downbeats
        .first()
        .and_then(|d| beats.iter().position(|b| (b - d).abs() < 1e-3))
        .unwrap_or(0);
    let mut down_idx = first_down;
    if opts.tempo_factor >= 1.5 {
        let mut v = Vec::with_capacity(beats.len() * 2);
        for w in beats.windows(2) {
            v.push(w[0]);
            v.push((w[0] + w[1]) / 2.0);
        }
        if let Some(l) = beats.last() {
            v.push(*l);
        }
        beats = v;
        down_idx *= 2;
    } else if opts.tempo_factor <= 0.75 {
        beats = beats
            .iter()
            .skip(down_idx % 2)
            .step_by(2)
            .copied()
            .collect();
        down_idx /= 2;
    }
    if beats.len() < 8 {
        return None;
    }
    let bpb = opts
        .beats_per_bar
        .or(sk.beats_per_bar)
        .unwrap_or(4)
        .clamp(1, 16);
    // 小節頭をずらす(手で直す)
    let down = (down_idx as i64 + opts.downbeat_shift as i64).rem_euclid(bpb as i64) as u32;
    let map = tempo_map(&beats);
    let first = map.first()?;
    if opts.no_pickup == Some(true) && down > 0 {
        // 弱起なし: 最初の小節頭(down 番目の拍)を曲の 0 にし、その前は切る
        let k = map.iter().rposition(|seg| seg.beat <= down as usize)?;
        let seg = &map[k];
        let t0 = seg.time + (down as usize - seg.beat) as f64 * 60.0 / seg.bpm;
        let mut tempo = vec![glaux_core::time::TempoEvent {
            tick: glaux_core::Tick(0),
            bpm: seg.bpm,
        }];
        for seg in map.iter().skip(k + 1) {
            tempo.push(glaux_core::time::TempoEvent {
                tick: glaux_core::Tick((seg.beat as u64 - down as u64) * ppq),
                bpm: seg.bpm,
            });
        }
        return Some(GridPlan {
            beats,
            beats_per_bar: bpb,
            lead_beats: -(down as i64),
            audio_start_sec: -t0,
            tempo,
        });
    }
    let bpm0 = first.bpm;
    // 最初の拍(beats[0])の前の拍: 最初の小節頭(down 番目の拍)を小節の頭にそろえる
    let mut lead = (bpb - down % bpb) % bpb;
    // 弱起の小節より前にも音(前奏)があるなら、それが曲の 0 秒より後に入るよう 1 小節ずつ足す。無ければその前を切る
    if opts.keep_intro == Some(true) {
        while (lead as f64) * 60.0 / bpm0 < first.time {
            lead += bpb;
        }
    }
    let audio_start_sec = lead as f64 * 60.0 / bpm0 - first.time;
    let mut tempo = vec![glaux_core::time::TempoEvent {
        tick: glaux_core::Tick(0),
        bpm: bpm0,
    }];
    for seg in map.iter().skip(1) {
        tempo.push(glaux_core::time::TempoEvent {
            tick: glaux_core::Tick((seg.beat as u64 + lead as u64) * ppq),
            bpm: seg.bpm,
        });
    }
    let lead = lead as i64;
    Some(GridPlan {
        beats,
        beats_per_bar: bpb,
        lead_beats: lead,
        audio_start_sec,
        tempo,
    })
}

/// 最初の小節頭より前が静か(曲全体より 20dB 以上小さい: 無音・ノイズ・拍の推定のはみ出し)なら、弱起なしにする
pub fn auto_no_pickup(sk: &Skeleton, mix: &[f32], sr: f32) -> bool {
    let Some(&d) = sk.downbeats.first() else {
        return false;
    };
    // 小節頭の拍の推定は 20ms 刻みで少し遅れることがあるので、小節頭の音の立ち上がりを含めないよう 60ms 手前まで
    let n = (((d - 0.06).max(0.0) * sr as f64) as usize).min(mix.len());
    if n < (0.05 * sr) as usize {
        return false;
    }
    let db = |x: &[f32]| {
        10.0 * (x.iter().map(|v| v * v).sum::<f32>() / x.len().max(1) as f32)
            .max(1e-12)
            .log10()
    };
    db(&mix[..n]) < db(mix) - 20.0
}

/// 格子を作る(弱起の有無が決まっていなければ、`auto_no_pickup` で決める)。決めた弱起の有無も返す
pub fn resolve_grid(
    sk: &Skeleton,
    mix: &[f32],
    sr: f32,
    opts: &GridOptions,
) -> Option<(GridPlan, bool)> {
    let mut o = *opts;
    if o.no_pickup.is_none() {
        o.no_pickup = Some(auto_no_pickup(sk, mix, sr));
    }
    if o.keep_intro.is_none() {
        // 弱起の小節の頭より前に音があるか(曲全体より 20dB 以上小さければ無いとみなす)
        let p = grid_plan(
            sk,
            &GridOptions {
                keep_intro: Some(false),
                ..o
            },
        )?;
        let n = ((((-p.audio_start_sec) - 0.06).max(0.0) * sr as f64) as usize).min(mix.len());
        let db = |x: &[f32]| {
            10.0 * (x.iter().map(|v| v * v).sum::<f32>() / x.len().max(1) as f32)
                .max(1e-12)
                .log10()
        };
        o.keep_intro = Some(n >= (0.05 * sr) as usize && db(&mix[..n]) >= db(mix) - 20.0);
    }
    let plan = grid_plan(sk, &o)?;
    let cut = plan.lead_beats < 0;
    Some((plan, cut))
}

/// 格子を曲に写すコマンド: テンポ・拍子を置き換え、参考曲のクリップを格子に合う位置・長さへ動かす
pub fn grid_commands(
    project: &glaux_core::Project,
    plan: &GridPlan,
) -> Result<Vec<glaux_core::Command>, String> {
    use glaux_core::{ClipContent, Command, Tick};
    let map = plan.tempo_map();
    let mut cmds = vec![
        Command::SetTempo {
            events: plan.tempo.clone(),
        },
        Command::SetTimeSig {
            events: vec![glaux_core::time::TimeSigEvent::new(
                Tick(0),
                plan.beats_per_bar as u8,
                4,
            )],
        },
    ];
    for t in project.tracks.iter().filter(|t| t.reference) {
        for c in &t.clips {
            // 骨組みは素材の頭(0 秒)から解析している。格子の前を切るときは、素材の読み始めをその分にする
            let ClipContent::Audio { asset, .. } = &c.content else {
                continue;
            };
            let Some(meta) = project.assets.get(asset) else {
                continue;
            };
            let rate = meta.sample_rate.max(1) as f64;
            let offset = (((-plan.audio_start_sec).max(0.0) * rate) as u64).min(meta.frames);
            let secs = meta.frames.saturating_sub(offset) as f64 / rate;
            let start_sec = plan.audio_start_sec.max(0.0);
            let start = map.seconds_to_tick(start_sec);
            let end = map.seconds_to_tick(start_sec + secs);
            let mut clip = c.clone();
            clip.start = start;
            clip.length = Tick(end.0.saturating_sub(start.0).max(1));
            if let ClipContent::Audio { offset_samples, .. } = &mut clip.content {
                *offset_samples = offset;
            }
            cmds.push(Command::ReplaceClip {
                id: c.id.clone(),
                clip,
            });
        }
    }
    Ok(cmds)
}

/// 起こすパート(作業場の「パート」の手順)
#[derive(Clone, Debug, serde::Deserialize, Serialize)]
pub struct PartSpec {
    /// トラックの名前
    pub name: String,
    pub kind: PartKind,
    /// GM の音色の番号(GM 音源があれば使う)
    #[serde(default)]
    pub program: Option<u16>,
}

/// パートのトラックを作るコマンド。音符の時刻(音声の秒)を格子で tick にし、`quantize`(0〜1)の強さで 16 分の格子へ寄せる。
/// `soundfont` があればその GM の音色、無ければ内蔵の音源で鳴らす
pub fn part_track_commands(
    plan: &GridPlan,
    part: &PartSpec,
    notes: &[RawNote],
    quantize: f64,
    soundfont: Option<&str>,
) -> Vec<glaux_core::Command> {
    use glaux_core::{
        Clip, ClipContent, ClipId, Command, Device, Note, NoteId, PluginSource, Tick, Track,
        TrackId, TrackKind,
    };
    let ppq = glaux_core::time::PPQ as f64;
    let map = plan.tempo_map();
    let grid = ppq / 4.0;
    let q = quantize.clamp(0.0, 1.0);
    let snap = |t: f64| -> f64 {
        let g = (t / grid).round() * grid;
        t + (g - t) * q
    };
    let mut out: Vec<Note> = notes
        .iter()
        .filter_map(|n| {
            // 曲の頭より前(弱起なしで切った所)の音は捨て、またがる音は頭から
            if plan.audio_start_sec + n.end <= 0.0 {
                return None;
            }
            let a = snap(plan.tick_of(&map, n.start.max(-plan.audio_start_sec))).max(0.0);
            let b = snap(plan.tick_of(&map, n.end)).max(a + if q > 0.5 { grid } else { 30.0 });
            (b > a).then(|| Note {
                locked: false,
                id: NoteId::new(),
                pos: Tick(a.round() as u64),
                dur: Tick((b - a).round().max(1.0) as u64),
                pitch: n.pitch,
                vel: n.vel.clamp(1, 127),
                articulation: Default::default(),
                pitch_curve: vec![],
                glide_ms: None,
                vibrato: None,
                volume_curve: vec![],
                brightness_curve: vec![],
                condition: None,
            })
        })
        .collect();
    out.sort_by_key(|n| (n.pos, n.pitch));
    // 同じ位置・同じ高さの重なりは 1 つに
    out.dedup_by(|b, a| a.pos == b.pos && a.pitch == b.pitch);
    let bar = ppq * plan.beats_per_bar as f64;
    let last = out.iter().map(|n| n.pos.0 + n.dur.0).max().unwrap_or(0) as f64;
    let length = ((last / bar).ceil().max(1.0) * bar) as u64;
    let tid = TrackId::new();
    let mut track = Track::new(tid.clone(), part.name.clone(), TrackKind::Midi);
    track.device = Some(match (soundfont, part.program) {
        (Some(sf), Some(p)) => Device {
            source: PluginSource::Sf2 {
                soundfont: sf.to_owned(),
                // ドラムは GM のドラムの音色(バンク 128)
                bank: if part.kind == PartKind::Drums { 128 } else { 0 },
                preset: p,
            },
            params: Default::default(),
        },
        _ => Device::builtin(match part.kind {
            PartKind::Bass => "subtractive",
            PartKind::Melody => "fm",
            PartKind::Poly => "pluck",
            PartKind::Drums => "drum",
        }),
    });
    let mut clip = Clip::new_midi(ClipId::new(), part.name.clone(), Tick(0), Tick(length));
    if let ClipContent::Midi { notes: n, .. } = &mut clip.content {
        *n = out;
    }
    vec![
        Command::AddTrack { track, index: None },
        Command::AddClip { track: tid, clip },
    ]
}

// ---- 作業場(画面と AI の道具が共用) ------------------------------------------

/// 参考曲のクリップの骨組み。素材の頭(0 秒)から最後までを解析する(時刻は素材の頭から。クリップの読み始めに左右されない)。
/// 同じ素材は解析し直さない。(骨組み, 素材のモノラルの音, サンプルレート)
pub fn clip_skeleton(
    project: &glaux_core::Project,
    dir: &std::path::Path,
    clip_id: &glaux_core::ClipId,
) -> Result<(std::sync::Arc<Skeleton>, Vec<f32>, f32), String> {
    use std::sync::{Arc, Mutex};
    type Entry = (String, Arc<Skeleton>, Arc<Vec<f32>>, f32);
    static CACHE: Mutex<Vec<Entry>> = Mutex::new(Vec::new());
    let clip = project
        .tracks
        .iter()
        .find_map(|t| t.clips.iter().find(|c| &c.id == clip_id))
        .ok_or_else(|| format!("クリップが見つかりません: {clip_id}"))?;
    let glaux_core::ClipContent::Audio { asset, .. } = &clip.content else {
        return Err(format!("{clip_id} は音声クリップではありません"));
    };
    let key = asset.to_string();
    if let Some((_, sk, frames, sr)) = CACHE
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .iter()
        .find(|e| e.0 == key)
    {
        return Ok((sk.clone(), frames.to_vec(), *sr));
    }
    let meta = project
        .assets
        .get(asset)
        .ok_or_else(|| format!("アセットが見つかりません: {asset}"))?;
    let data = glaux_engine::load_wav_mono(&dir.join(&meta.path))?;
    let sk = Arc::new(analyze(&data.frames, data.sample_rate, Default::default())?);
    let mut cache = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    cache.insert(
        0,
        (
            key,
            sk.clone(),
            Arc::new(data.frames.clone()),
            data.sample_rate,
        ),
    );
    cache.truncate(3);
    Ok((sk, data.frames, data.sample_rate))
}

/// 格子の小節頭の音声の時刻(秒)と小節の番号(1 始まり)。弱起の小節(頭が音声の前にある小節)も含む
pub fn grid_bars(plan: &GridPlan) -> Vec<(u32, f64)> {
    let map = plan.tempo_map();
    let bar = glaux_core::time::PPQ * plan.beats_per_bar.max(1) as u64;
    let end = plan.beats.last().copied().unwrap_or(0.0) + 1.0;
    let mut out = Vec::new();
    for n in 0u32.. {
        let t = map.tick_to_seconds(glaux_core::Tick(n as u64 * bar)) - plan.audio_start_sec;
        if t > end || n > 5000 {
            break;
        }
        let next =
            map.tick_to_seconds(glaux_core::Tick((n as u64 + 1) * bar)) - plan.audio_start_sec;
        if next > 0.0 {
            out.push((n + 1, t));
        }
    }
    out
}

/// 作業場の画面に出す骨組みと格子(拍・小節頭・小節ごとの和音・テンポ)
pub fn workspace_json(sk: &Skeleton, plan: &GridPlan, no_pickup: bool) -> serde_json::Value {
    use serde_json::json;
    let r3 = |v: f64| (v * 1000.0).round() / 1000.0;
    let bars = grid_bars(plan);
    let bounds: Vec<(f64, f64)> = bars
        .iter()
        .enumerate()
        .map(|(k, &(_, t))| {
            let end = bars.get(k + 1).map_or(sk.duration_sec, |b| b.1);
            (t.max(0.0), end)
        })
        .collect();
    let names = bar_chord_names(&sk.chords, &bounds);
    json!({
        "duration_sec": r3(sk.duration_sec),
        "beats": plan.beats.iter().take(4000).map(|b| r3(*b)).collect::<Vec<_>>(),
        "bars": bars.iter().zip(&names).take(1000).map(|(&(n, t), c)| json!({"bar": n, "time": r3(t), "chords": c})).collect::<Vec<_>>(),
        "beats_per_bar": plan.beats_per_bar,
        "lead_beats": plan.lead_beats,
        "no_pickup": no_pickup,
        "audio_start_sec": r3(plan.audio_start_sec),
        "tempo": plan.tempo.iter().map(|e| json!({"tick": e.tick.0, "bpm": e.bpm})).collect::<Vec<_>>(),
        "bpm": sk.bpm,
        "key": sk.key,
        "tuning_cents": sk.tuning_cents,
        "groove": sk.groove,
        "summary": skeleton_json(sk, "", None)["summary"],
    })
}

/// 区間の印(構成の解析から): 格子の小節頭で区切って、区間の頭に役割の名前を置く
pub fn section_commands(frames: &[f32], sr: f32, plan: &GridPlan) -> Vec<glaux_core::Command> {
    let bars = grid_bars(plan);
    let downs: Vec<f64> = bars.iter().map(|b| b.1).filter(|t| *t >= 0.0).collect();
    let Ok(st) = glaux_engine::structure::analyze(frames, sr, &downs) else {
        return Vec::new();
    };
    let map = plan.tempo_map();
    let sections = st
        .sections
        .iter()
        .filter_map(|s| {
            let t = st.bar_bounds.get(s.start_bar - 1)?.0;
            let tick = plan.tick_of(&map, t).round().max(0.0) as u64;
            // 小節の頭へ寄せる
            let bar = glaux_core::time::PPQ * plan.beats_per_bar as u64;
            let tick = (tick + bar / 2) / bar * bar;
            Some(glaux_core::SectionMarker {
                tick: glaux_core::Tick(tick),
                name: if s.role_ja.is_empty() {
                    s.label.clone()
                } else {
                    s.role_ja.to_owned()
                },
                ..Default::default()
            })
        })
        .collect::<Vec<_>>();
    if sections.is_empty() {
        return Vec::new();
    }
    vec![glaux_core::Command::SetSections { sections }]
}

/// 起こすパートの頼み(音の素: None なら参考曲そのもの、Some ならそのパートだけを分けた音のファイル。参考曲の素材と頭がそろっていること)
#[derive(Clone, Debug, serde::Deserialize, Serialize)]
pub struct PartRequest {
    #[serde(flatten)]
    pub spec: PartSpec,
    #[serde(default)]
    pub source: Option<String>,
}

/// 参考曲を分離ツール(利用者が入れた Demucs)で分けた音のファイル
#[derive(Clone, Debug, Default, Serialize)]
pub struct SeparatedFiles {
    pub vocals: Option<String>,
    pub drums: Option<String>,
    pub bass: Option<String>,
    pub other: Option<String>,
    /// 旋律を起こす音: 歌があれば歌、ほぼ空(楽器だけの曲)なら「その他」(旋律の楽器はそこに入る)
    pub melody: Option<String>,
}

/// 分けた歌が「その他」よりこれ以上小さければ、歌は無いとみなす(dB)
const NO_VOCALS_DB: f32 = 20.0;

/// 旋律を「その他」から起こすか(分けた歌が「その他」より NO_VOCALS_DB 以上小さい = 楽器だけの曲)
pub fn melody_from_other(vocals: &[f32], other: &[f32]) -> bool {
    let rms = |x: &[f32]| -> f32 {
        let e = x.iter().map(|v| v * v).sum::<f32>() / x.len().max(1) as f32;
        10.0 * (e + 1e-12).log10()
    };
    rms(vocals) < rms(other) - NO_VOCALS_DB
}

/// 参考曲の素材(頭から最後まで)を Demucs で 4 つ(歌・ドラム・ベース・その他)に分け、曲のフォルダの
/// `cache/recon/<素材>/` に置く。一度分けた素材は分け直さない。Demucs が無ければエラー(入れ方の案内つき)
pub fn separate_reference(
    project: &glaux_core::Project,
    dir: &std::path::Path,
    clip_id: &glaux_core::ClipId,
) -> Result<SeparatedFiles, String> {
    let clip = project
        .tracks
        .iter()
        .find_map(|t| t.clips.iter().find(|c| &c.id == clip_id))
        .ok_or_else(|| format!("クリップが見つかりません: {clip_id}"))?;
    let glaux_core::ClipContent::Audio { asset, .. } = &clip.content else {
        return Err(format!("{clip_id} は音声クリップではありません"));
    };
    let meta = project
        .assets
        .get(asset)
        .ok_or_else(|| format!("アセットが見つかりません: {asset}"))?;
    let out = dir
        .join("cache")
        .join("recon")
        .join(asset.to_string().replace(':', "_"));
    let names = ["vocals", "drums", "bass", "other"];
    let path = |n: &str| out.join(format!("{n}.wav"));
    let files = || -> SeparatedFiles {
        let p = |n: &str| Some(path(n).to_string_lossy().into_owned());
        let mono = |n: &str| {
            glaux_engine::load_wav_mono(&path(n))
                .map(|d| d.frames)
                .unwrap_or_default()
        };
        let melody = if melody_from_other(&mono("vocals"), &mono("other")) {
            p("other")
        } else {
            p("vocals")
        };
        SeparatedFiles {
            vocals: p("vocals"),
            drums: p("drums"),
            bass: p("bass"),
            other: p("other"),
            melody,
        }
    };
    if names.iter().all(|n| path(n).is_file()) {
        return Ok(files());
    }
    let data = glaux_engine::load_wav(&dir.join(&meta.path))?;
    std::fs::create_dir_all(&out).map_err(|e| e.to_string())?;
    let stems =
        crate::stems::run_demucs(&data.frames, data.side.as_deref(), data.sample_rate, &out)?;
    // run_demucs は 歌・ドラム・ベース・その他 の順
    for (n, (_, mid, side)) in names.iter().zip(&stems) {
        crate::stems::write_wav_ms(&path(n), mid, side.as_deref(), data.sample_rate as u32)?;
    }
    Ok(files())
}

/// 分離ツール(Demucs)がこの PC に入っているか
pub fn separator_available() -> bool {
    crate::stems::find_demucs().is_some()
}

/// 起こした結果: (コマンド, パートごとの (名前, 音符の数))
pub type TranscribedParts = (Vec<glaux_core::Command>, Vec<(String, usize)>);

/// 参考曲からパートを起こし、格子とトラックを置くコマンドにする。(コマンド, パートごとの音符の数)
pub fn transcribe_commands(
    project: &glaux_core::Project,
    dir: &std::path::Path,
    clip_id: &glaux_core::ClipId,
    opts: &GridOptions,
    parts: &[PartRequest],
    quantize: f64,
) -> Result<TranscribedParts, String> {
    let (sk, mix, sr) = clip_skeleton(project, dir, clip_id)?;
    let (plan, _) = resolve_grid(&sk, &mix, sr, opts).ok_or("拍が少なすぎて格子を作れません")?;
    let mut cmds = grid_commands(project, &plan)?;
    let sf = glaux_engine::sf2::default_dir().join(crate::models::SOUNDFONT_FILE);
    let soundfont = sf.is_file().then_some(crate::models::SOUNDFONT_FILE);
    let notes = transcribe_requests(&sk, &mix, sr, parts)?;
    let mut counts = Vec::new();
    for (p, notes) in parts.iter().zip(&notes) {
        counts.push((p.spec.name.clone(), notes.len()));
        cmds.extend(part_track_commands(
            &plan, &p.spec, notes, quantize, soundfont,
        ));
    }
    Ok((cmds, counts))
}

/// 頼まれたパートを起こす(音の素ごとに)。参考曲そのものから起こすパートは、1 回の譜起こしを声部に分ける
/// (旋律 = 上、ベース = 下、和音 = 残り)。時刻は参考曲の素材の秒
pub fn transcribe_requests(
    sk: &Skeleton,
    mix: &[f32],
    sr: f32,
    parts: &[PartRequest],
) -> Result<Vec<Vec<RawNote>>, String> {
    let from_mix: Vec<PartKind> = parts
        .iter()
        .filter(|p| p.source.is_none() && p.spec.kind != PartKind::Drums)
        .map(|p| p.spec.kind)
        .collect();
    let mut mix_parts = if from_mix.is_empty() {
        Vec::new()
    } else {
        transcribe_mix_parts(mix, sr, &from_mix, sk.tuning_cents)?
    }
    .into_iter();
    parts
        .iter()
        .map(|p| match &p.source {
            None if p.spec.kind == PartKind::Drums => {
                transcribe_part(mix, sr, PartKind::Drums, NoteMethod::DrumsMix, 0.0)
            }
            Some(path) => {
                let s = crate::sound::load_file(std::path::Path::new(path))?;
                transcribe_part(
                    &s.frames,
                    s.sample_rate,
                    p.spec.kind,
                    p.spec.kind.default_method(),
                    sk.tuning_cents,
                )
            }
            None => Ok(mix_parts.next().unwrap_or_default()),
        })
        .collect()
}

// ---- 今の曲に取り込む(段階 3) -------------------------------------------------

/// 取り込み方
#[derive(Clone, Debug, serde::Deserialize, Serialize)]
pub struct ImportOptions {
    /// 参考曲の小節の範囲(1 始まり、両端を含む。格子の小節の番号)
    pub from_bar: u32,
    pub to_bar: u32,
    /// 今の曲のどの小節から置くか(1 始まり)
    pub at_bar: u32,
    /// 移調(半音)。None なら今の曲の調へ合わせる(今の曲に音が無ければそのまま)
    #[serde(default)]
    pub transpose: Option<i32>,
    /// 置き先のトラック(パートが 1 つのとき。None なら新しいトラック)
    #[serde(default)]
    pub track_id: Option<glaux_core::TrackId>,
}

/// 参考曲の素材の秒 → 曲の拍(1 小節目の頭を 0。格子の拍を線形にたどり、端の外は端の拍の長さで延ばす)
fn song_beat(plan: &GridPlan, t: f64) -> f64 {
    let b = &plan.beats;
    let n = b.len();
    let idx = if n < 2 {
        0.0
    } else if t <= b[0] {
        (t - b[0]) / (b[1] - b[0])
    } else if t >= b[n - 1] {
        (n - 1) as f64 + (t - b[n - 1]) / (b[n - 1] - b[n - 2])
    } else {
        let i = b.partition_point(|x| *x <= t) - 1;
        i as f64 + (t - b[i]) / (b[i + 1] - b[i])
    };
    idx + plan.lead_beats as f64
}

/// 参考曲の調を今の曲の調へ移す半音(-5〜+6)。長調と短調が違えば、平行調(短調なら 3 半音下が長調の主音)で合わせる
pub fn auto_transpose(sk: &Skeleton, project: &glaux_core::Project) -> i32 {
    let (Some(r), Some(k)) = (
        sk.key.as_ref(),
        glaux_core::harmony::analyze(project, None, None).key,
    ) else {
        return 0;
    };
    let song_minor = k.mode == "minor";
    // 今の曲の、参考曲と同じ旋法の主音
    let target = match (r.minor, song_minor) {
        (true, false) => (k.tonic + 9) % 12,
        (false, true) => (k.tonic + 3) % 12,
        _ => k.tonic,
    };
    let d = (target as i32 - r.tonic as i32).rem_euclid(12);
    if d > 6 {
        d - 12
    } else {
        d
    }
}

/// 取り込んだ結果: (コマンド, パートごとの (名前, 音符の数), 移調した半音)
pub type ImportedParts = (Vec<glaux_core::Command>, Vec<(String, usize)>, i32);

/// 参考曲の一部のパートを今の曲に取り込むコマンド(今の曲のテンポ・拍子は変えない。拍の位置で写す)。
/// (コマンド, パートごとの (名前, 音符の数), 移調した半音)
pub fn import_commands(
    project: &glaux_core::Project,
    dir: &std::path::Path,
    clip_id: &glaux_core::ClipId,
    grid: &GridOptions,
    parts: &[PartRequest],
    quantize: f64,
    import: &ImportOptions,
) -> Result<ImportedParts, String> {
    use glaux_core::{
        Clip, ClipContent, ClipId, Command, Device, PluginSource, Tick, Track, TrackId, TrackKind,
    };
    if import.to_bar < import.from_bar || import.from_bar == 0 || import.at_bar == 0 {
        return Err("小節の範囲が正しくありません(from_bar ≦ to_bar、1 始まり)".to_owned());
    }
    if import.track_id.is_some() && parts.len() != 1 {
        return Err("置き先のトラックを指定するときは、パートを 1 つにしてください".to_owned());
    }
    let (sk, mix, sr) = clip_skeleton(project, dir, clip_id)?;
    let (plan, _) = resolve_grid(&sk, &mix, sr, grid).ok_or("拍が少なすぎて格子を作れません")?;
    let bpb = plan.beats_per_bar as f64;
    let (b0, b1) = (
        (import.from_bar - 1) as f64 * bpb,
        import.to_bar as f64 * bpb,
    );
    // 置き先の小節の頭(今の曲の拍子で。曲の終わりより先は最後の小節の長さで延ばす)
    let ppq = glaux_core::time::PPQ as f64;
    let grid_bars =
        glaux_core::arrange::bar_grid(project, (import.at_bar as u64 + 1) * 4 * 4 * ppq as u64);
    let at = match grid_bars.get(import.at_bar as usize - 1) {
        Some(&(t, _)) => t,
        None => {
            let (t, len) = grid_bars.last().copied().unwrap_or((0, 4 * ppq as u64));
            t + len * (import.at_bar as u64 - grid_bars.len() as u64)
        }
    };
    let transpose = import
        .transpose
        .unwrap_or_else(|| auto_transpose(&sk, project));
    let length = ((b1 - b0) * ppq).round() as u64;
    let q = quantize.clamp(0.0, 1.0);
    let step = ppq / 4.0;
    let snap = |t: f64| -> f64 {
        let g = (t / step).round() * step;
        t + (g - t) * q
    };
    let sf = glaux_engine::sf2::default_dir().join(crate::models::SOUNDFONT_FILE);
    let soundfont = sf.is_file().then_some(crate::models::SOUNDFONT_FILE);
    let notes = transcribe_requests(&sk, &mix, sr, parts)?;
    let mut cmds = Vec::new();
    let mut counts = Vec::new();
    for (p, raw) in parts.iter().zip(&notes) {
        // ドラムの音の番号は打楽器の種類なので移調しない
        let shift = if p.spec.kind == PartKind::Drums {
            0
        } else {
            transpose
        };
        let mut out: Vec<glaux_core::Note> = raw
            .iter()
            .filter_map(|n| {
                let (a, e) = (song_beat(&plan, n.start), song_beat(&plan, n.end));
                // 範囲の頭より前に始まる音は除き、範囲の終わりで切る
                if a < b0 - 0.05 || a >= b1 {
                    return None;
                }
                let pos = snap((a - b0).max(0.0) * ppq);
                let end = snap((e.min(b1) - b0) * ppq).max(pos + if q > 0.5 { step } else { 30.0 });
                let pitch = (n.pitch as i32 + shift).clamp(0, 127) as u8;
                Some(glaux_core::Note {
                    locked: false,
                    id: glaux_core::NoteId::new(),
                    pos: Tick(pos.round() as u64),
                    dur: Tick((end - pos).round().max(1.0) as u64),
                    pitch,
                    vel: n.vel.clamp(1, 127),
                    articulation: Default::default(),
                    pitch_curve: vec![],
                    glide_ms: None,
                    vibrato: None,
                    volume_curve: vec![],
                    brightness_curve: vec![],
                    condition: None,
                })
            })
            .collect();
        out.sort_by_key(|n| (n.pos, n.pitch));
        out.dedup_by(|b, a| a.pos == b.pos && a.pitch == b.pitch);
        counts.push((p.spec.name.clone(), out.len()));
        let name = glaux_core::tr!(
            "{}(参考曲 {}〜{} 小節)",
            "{} (reference bars {}–{})",
            p.spec.name,
            import.from_bar,
            import.to_bar
        );
        let mut clip = Clip::new_midi(ClipId::new(), name.clone(), Tick(at), Tick(length.max(1)));
        if let ClipContent::Midi { notes, .. } = &mut clip.content {
            *notes = out;
        }
        let tid = match &import.track_id {
            Some(t) => {
                if project.track(t).is_none_or(|t| t.kind != TrackKind::Midi) {
                    return Err(format!("置き先は MIDI トラックにしてください: {t}"));
                }
                t.clone()
            }
            None => {
                let tid = TrackId::new();
                let mut track = Track::new(tid.clone(), p.spec.name.clone(), TrackKind::Midi);
                track.device = Some(match (soundfont, p.spec.program) {
                    (Some(sf), Some(pr)) => Device {
                        source: PluginSource::Sf2 {
                            soundfont: sf.to_owned(),
                            bank: if p.spec.kind == PartKind::Drums {
                                128
                            } else {
                                0
                            },
                            preset: pr,
                        },
                        params: Default::default(),
                    },
                    _ => Device::builtin(match p.spec.kind {
                        PartKind::Bass => "subtractive",
                        PartKind::Melody => "fm",
                        PartKind::Poly => "pluck",
                        PartKind::Drums => "drum",
                    }),
                });
                cmds.push(Command::AddTrack { track, index: None });
                tid
            }
        };
        cmds.push(Command::AddClip { track: tid, clip });
    }
    Ok((cmds, counts, transpose))
}

/// 小節(`bounds` の (開始, 終了) 秒)ごとの和音の名前(区間に 1/8 以上かかる和音を順に。"C G/B")。和音が無ければ "N.C."
pub fn bar_chord_names(chords: &[ChordSpan], bounds: &[(f64, f64)]) -> Vec<String> {
    bounds
        .iter()
        .map(|&(s, e)| {
            let min = (e - s) / 8.0;
            let mut names: Vec<&str> = Vec::new();
            for c in chords {
                let ov = e.min(c.end) - s.max(c.start);
                if ov >= min && c.symbol != "N" && names.last() != Some(&c.symbol.as_str()) {
                    names.push(&c.symbol);
                }
            }
            if names.is_empty() {
                "N.C.".to_owned()
            } else {
                names.join(" ")
            }
        })
        .collect()
}

/// 小節の (開始, 終了) 秒。小節頭から(最初の小節頭より前に拍があれば 0 小節目〈弱起〉も)
fn bars_of(sk: &Skeleton) -> Vec<(usize, f64, f64)> {
    let d = &sk.downbeats;
    let mut out = Vec::new();
    if d.is_empty() {
        return out;
    }
    if sk.beats.first().is_some_and(|b| *b < d[0] - 0.05) {
        out.push((0, sk.beats[0].min(d[0]), d[0]));
    }
    for (i, &t) in d.iter().enumerate() {
        let end = d.get(i + 1).copied().unwrap_or_else(|| {
            let len = if i > 0 { t - d[i - 1] } else { 2.0 };
            (t + len).min(sk.duration_sec)
        });
        if end > t {
            out.push((i + 1, t, end));
        }
    }
    out
}

/// 曲の骨組みを AI 向けの JSON にする(小節ごとの和音・テンポの変化を小節と拍の位置で)
/// `audio`(解析した音)を渡すと、小節の番号を作業場の格子と同じ数え方にする(弱起の有無を音から決める)
pub fn skeleton_json(
    sk: &Skeleton,
    source: &str,
    audio: Option<(&[f32], f32)>,
) -> serde_json::Value {
    use serde_json::json;
    let r2 = |v: f64| (v * 100.0).round() / 100.0;
    let r3 = |v: f64| (v * 1000.0).round() / 1000.0;
    let grid = audio.and_then(|(f, sr)| resolve_grid(sk, f, sr, &GridOptions::default()));
    let bars: Vec<(usize, f64, f64)> = match &grid {
        Some((plan, _)) => {
            let g = grid_bars(plan);
            g.iter()
                .enumerate()
                .map(|(k, &(n, t))| {
                    let end = g.get(k + 1).map_or(sk.duration_sec, |b| b.1);
                    (n as usize, t.max(0.0), end)
                })
                .collect()
        }
        None => bars_of(sk),
    };
    let bounds: Vec<(f64, f64)> = bars.iter().map(|b| (b.1, b.2)).collect();
    let names = bar_chord_names(&sk.chords, &bounds);
    // 時刻 → (小節, 拍)
    let bar_beat = |t: f64| -> (usize, usize) {
        let k = bars.iter().rposition(|b| b.1 <= t + 1e-3).unwrap_or(0);
        let start = bars.get(k).map_or(0.0, |b| b.1);
        let beat = sk
            .beats
            .iter()
            .filter(|&&b| b >= start - 1e-3 && b < t - 1e-3)
            .count();
        (bars.get(k).map_or(0, |b| b.0), beat + 1)
    };
    let tempo: Vec<serde_json::Value> = sk
        .tempo_map
        .iter()
        .map(|seg| {
            let (bar, beat) = bar_beat(seg.time);
            json!({"bar": bar, "beat": beat, "time_sec": r3(seg.time), "bpm": seg.bpm})
        })
        .collect();
    const MAX_BARS: usize = 300;
    let bar_list: Vec<serde_json::Value> = bars
        .iter()
        .zip(&names)
        .take(MAX_BARS)
        .map(|(b, n)| json!({"bar": b.0, "start_sec": r2(b.1), "chords": n}))
        .collect();
    let changes = sk.tempo_map.len().saturating_sub(1);
    let summary = format!(
        "{}{}{}{}。{}{}",
        sk.bpm
            .map(|b| format!("約 {b} BPM"))
            .unwrap_or_else(|| "テンポ不明".to_owned()),
        sk.beats_per_bar
            .map(|n| format!("・1 小節 {n} 拍"))
            .unwrap_or_default(),
        sk.key
            .as_ref()
            .map(|k| format!("・{}(確からしさ {:.0}%)", k.name, k.confidence * 100.0))
            .unwrap_or_default(),
        if sk.tuning_cents.abs() >= 5.0 {
            format!("・チューニングが {:+.0} セントずれている", sk.tuning_cents)
        } else {
            String::new()
        },
        if changes == 0 {
            "テンポは一定".to_owned()
        } else if changes <= 3 {
            format!("テンポが {changes} 回変わる")
        } else {
            format!("テンポが細かく揺れる・だんだん変わる({} 区間)", changes + 1)
        },
        sk.groove
            .as_ref()
            .filter(|g| !g.summary.is_empty())
            .map(|g| format!("。ノリ: {}", g.summary))
            .unwrap_or_default(),
    );
    json!({
        "source": source,
        "duration_sec": r2(sk.duration_sec),
        "bpm": sk.bpm,
        "beats_per_bar": sk.beats_per_bar,
        "first_downbeat_sec": sk.downbeats.first().map(|d| r3(*d)),
        "no_pickup": grid.as_ref().map(|g| g.1),
        "tuning_cents": sk.tuning_cents,
        "key": sk.key,
        "tempo_map": tempo,
        "bars": bar_list,
        "bars_total": bars.len(),
        "groove": sk.groove,
        "summary": summary,
    })
}

/// 自分の曲(解析と同じ条件で描き出したステレオ)のノリ。拍は曲のテンポの格子から
pub fn song_groove(
    project: &glaux_core::Project,
    stereo: &[f32],
    start_sec: f64,
) -> Option<glaux_engine::feel::Groove> {
    let mono: Vec<f32> = stereo.chunks(2).map(|c| (c[0] + c[1]) * 0.5).collect();
    let sr = 48_000.0f32;
    let dur = mono.len() as f64 / sr as f64;
    let tm = &project.tempo_map;
    let ppq = glaux_core::time::PPQ;
    let first = (tm.seconds_to_tick_f64(start_sec) / ppq as f64).floor() as u64;
    let beats: Vec<f64> = (first..)
        .map(|b| tm.tick_to_seconds(glaux_core::Tick(b * ppq)) - start_sec)
        .skip_while(|t| *t < 0.0)
        .take_while(|t| *t < dur)
        .collect();
    let onsets = glaux_engine::feel::onsets(&mono, sr);
    glaux_engine::feel::analyze(&onsets, &beats)
}

/// 参考曲と曲の統合ラウドネス(LUFS)を、同じ範囲で測る。範囲は曲の秒(None なら参考曲のクリップの範囲)。
/// 参考曲は、参考曲のトラックの最初の音声クリップ。(曲, 参考曲)。測れなければ None
pub fn reference_loudness(
    project: &glaux_core::Project,
    dir: &std::path::Path,
    range: Option<(f64, f64)>,
) -> Result<(Option<f64>, Option<f64>), String> {
    use glaux_core::ClipContent;
    let clip = project
        .tracks
        .iter()
        .filter(|t| t.reference)
        .flat_map(|t| t.clips.iter())
        .find(|c| matches!(c.content, ClipContent::Audio { .. }))
        .ok_or_else(|| "参考曲のトラックがありません".to_owned())?;
    let tm = &project.tempo_map;
    let (c0, c1) = (
        tm.tick_to_seconds(clip.start),
        tm.tick_to_seconds(clip.start + clip.length),
    );
    let (from, to) = match range {
        Some((a, b)) => (a.max(c0), b.min(c1)),
        None => (c0, c1),
    };
    if to - from < 1.0 {
        return Err("参考曲と重なる範囲が短すぎます(1 秒以上)".to_owned());
    }
    let rate = crate::sound::RENDER_RATE;
    let all = crate::sound::load_stereo_source(
        project,
        dir,
        &crate::sound::SoundSource::Clip(clip.id.clone()),
    )?;
    let a = (((from - c0) * rate) as usize * 2).min(all.len());
    let b = (((to - c0) * rate) as usize * 2).min(all.len());
    let reference = &all[a..b];
    let bank = glaux_engine::SampleBank::for_offline(project, dir);
    let span = (tm.seconds_to_tick(from), tm.seconds_to_tick(to));
    // 曲がまだ空(参考曲しか無い)なら曲側は測れない
    let song =
        glaux_engine::analyze::render_song_for_analysis(project, None, Some(span), &bank).ok();
    let lufs = |x: &[f32]| {
        let v = glaux_engine::export::lufs_at(x, rate as u32);
        v.is_finite().then_some((v * 10.0).round() / 10.0)
    };
    Ok((song.as_deref().and_then(lufs), lufs(reference)))
}

/// 拍ごとの区間(秒)と、区間の頭が小節の何拍目か(0 始まり)。最初の拍の前・最後の拍の後も区間にする
fn beat_segments(
    beats: &[f64],
    downbeats: &[f64],
    dur: f64,
) -> (Vec<(f64, f64)>, Vec<Option<u32>>) {
    let mut bounds = Vec::new();
    let mut pos = Vec::new();
    if beats.is_empty() {
        // 拍が無い: 1 秒ずつ
        let n = dur.ceil().max(1.0) as usize;
        for i in 0..n {
            bounds.push((i as f64, ((i + 1) as f64).min(dur)));
            pos.push(None);
        }
        return (bounds, pos);
    }
    if beats[0] > 0.05 {
        bounds.push((0.0, beats[0]));
        pos.push(None);
    }
    let mut count: Option<u32> = None;
    for (i, &b) in beats.iter().enumerate() {
        let end = beats.get(i + 1).copied().unwrap_or_else(|| {
            // 最後の拍は直前の拍の長さで閉じる
            let ibi = if i > 0 { b - beats[i - 1] } else { 0.5 };
            (b + ibi).min(dur)
        });
        if downbeats.iter().any(|d| (d - b).abs() < 1e-3) {
            count = Some(0);
        } else if let Some(c) = count.as_mut() {
            *c += 1;
        }
        if end > b {
            bounds.push((b, end));
            pos.push(count);
        }
    }
    if let Some(&(_, last)) = bounds.last() {
        if dur - last > 0.05 {
            bounds.push((last, dur));
            pos.push(None);
        }
    }
    (bounds, pos)
}

/// 拍の間隔の中央値からのテンポ
fn median_bpm(beats: &[f64]) -> Option<f64> {
    let mut ibi: Vec<f64> = beats
        .windows(2)
        .map(|w| w[1] - w[0])
        .filter(|d| *d > 0.0)
        .collect();
    if ibi.is_empty() {
        return None;
    }
    ibi.sort_by(f64::total_cmp);
    Some((60.0 / ibi[ibi.len() / 2] * 100.0).round() / 100.0)
}

/// テンポの区間に入れてよい、拍のずれの上限(秒)。拍の当たりの幅(70ms)より十分狭く
const TEMPO_TOL: f64 = 0.045;

/// 拍の時刻から、区間ごとに一定のテンポの並びを作る。区間の中の拍を最小二乗の直線(前の区間の終わりに
/// つなげる。最初の区間だけ切片も合わせる)で表し、ずれが [`TEMPO_TOL`] を超えない限り区間を伸ばす。
/// 演奏の細かな揺れや拍の推定の誤差は 1 つの区間にまとめ、テンポの変化(段差・だんだん遅く)は区間を分けて追う。
/// 区間の境目で時刻が飛ばない(曲のテンポの変化としてそのまま書ける)
pub fn tempo_map(beats: &[f64]) -> Vec<TempoSegment> {
    let n = beats.len();
    if n < 2 {
        return Vec::new();
    }
    // 区間 s..=e を直線で表す。anchor があれば s でその時刻を通る。(s での時刻, 1 拍の秒) とずれの最大
    let fit = |s: usize, e: usize, anchor: Option<f64>| -> Option<(f64, f64, f64)> {
        let (a, m) = match anchor {
            Some(a) => {
                let (mut num, mut den) = (0.0, 0.0);
                for (x, b) in beats[s + 1..=e].iter().enumerate() {
                    let x = (x + 1) as f64;
                    num += x * (b - a);
                    den += x * x;
                }
                (a, num / den)
            }
            None => {
                let cnt = (e - s + 1) as f64;
                let mx = (e - s) as f64 / 2.0;
                let my = beats[s..=e].iter().sum::<f64>() / cnt;
                let (mut sxy, mut sxx) = (0.0, 0.0);
                for (x, b) in beats[s..=e].iter().enumerate() {
                    let x = x as f64 - mx;
                    sxy += x * (b - my);
                    sxx += x * x;
                }
                let m = sxy / sxx;
                (my - m * mx, m)
            }
        };
        if m <= 0.0 {
            return None;
        }
        let worst = (s..=e)
            .map(|k| (beats[k] - (a + (k - s) as f64 * m)).abs())
            .fold(0.0, f64::max);
        Some((a, m, worst))
    };
    let mut out = Vec::new();
    let mut s = 0;
    let mut anchor: Option<f64> = None;
    while s < n - 1 {
        let mut best = None;
        for e in s + 1..n {
            match fit(s, e, anchor) {
                Some((a, m, worst)) if worst <= TEMPO_TOL || e == s + 1 => best = Some((e, a, m)),
                _ => break,
            }
        }
        let Some((e, a, m)) = best else {
            break;
        };
        out.push(TempoSegment {
            beat: s,
            time: (a * 1000.0).round() / 1000.0,
            bpm: (60.0 / m * 100.0).round() / 100.0,
        });
        anchor = Some(a + (e - s) as f64 * m);
        s = e;
    }
    out
}

/// テンポの並びから、`count` 個の拍の時刻を作る(最初の区間の頭から)
pub fn grid_beats(map: &[TempoSegment], count: usize) -> Vec<f64> {
    let mut out = Vec::with_capacity(count);
    let Some(first) = map.first() else {
        return out;
    };
    let mut seg = 0;
    let mut t = first.time;
    for i in first.beat..first.beat + count {
        while seg + 1 < map.len() && map[seg + 1].beat <= i {
            seg += 1;
            // 区間の頭は拍の時刻そのもの(丸めの誤差をためない)
            t = map[seg].time;
        }
        out.push(t);
        t += 60.0 / map[seg].bpm;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn melody_comes_from_other_when_there_are_no_vocals() {
        let tone =
            |a: f32| -> Vec<f32> { (0..4800).map(|i| a * (i as f32 * 0.05).sin()).collect() };
        // 楽器だけの曲: 歌はほぼ空
        assert!(melody_from_other(&tone(0.001), &tone(0.3)));
        // 歌のある曲
        assert!(!melody_from_other(&tone(0.2), &tone(0.3)));
    }

    #[test]
    fn note_ends_follow_the_sound() {
        // 0.1 秒で切れる A4 と、1 秒続く E4(basic-pitch が残響などで長く伸ばした想定で、どちらも終わりを長めに渡す)
        let sr = 22_050.0f32;
        let mut x = vec![0.0f32; (1.6 * sr) as usize];
        for (i, v) in x.iter_mut().enumerate() {
            let t = i as f32 / sr;
            if t < 0.1 {
                *v += 0.4 * (std::f32::consts::TAU * 440.0 * t).sin();
            }
            if (0.4..1.4).contains(&t) {
                *v += 0.4 * (std::f32::consts::TAU * 329.63 * t).sin();
            }
        }
        let notes = vec![
            RawNote {
                start: 0.0,
                end: 0.5,
                pitch: 69,
                vel: 100,
            },
            RawNote {
                start: 0.4,
                end: 1.45,
                pitch: 64,
                vel: 100,
            },
        ];
        let out = refine_note_ends(notes, &x, sr);
        // 切れた音は鳴り止んだあたりで終わる
        assert!(out[0].end > 0.08 && out[0].end < 0.2, "{:?}", out[0]);
        // 続く音は鳴っている間は短くしない
        assert!(out[1].end > 1.3, "{:?}", out[1]);
    }

    #[test]
    fn steady_beats_make_one_segment_and_changes_split() {
        // 120 BPM で人の揺れ(±15ms)
        let jit = [0.01, -0.015, 0.0, 0.012, -0.008, 0.015, -0.01, 0.0];
        let beats: Vec<f64> = (0..64).map(|i| 1.0 + i as f64 * 0.5 + jit[i % 8]).collect();
        let m = tempo_map(&beats);
        assert_eq!(m.len(), 1, "{m:?}");
        assert!((m[0].bpm - 120.0).abs() < 0.5);
        // 32 拍目から 100 BPM
        let mut b: Vec<f64> = (0..32).map(|i| i as f64 * 0.5).collect();
        let t0 = 32.0 * 0.5;
        b.extend((0..32).map(|i| t0 + i as f64 * 0.6));
        let m = tempo_map(&b);
        assert_eq!(m.len(), 2, "{m:?}");
        // 32 拍目(16.0 秒)までは 0.5 秒刻み、そこから 0.6 秒刻み
        assert_eq!(m[1].beat, 32);
        assert!((m[0].bpm - 120.0).abs() < 0.5 && (m[1].bpm - 100.0).abs() < 0.5);
        let g = grid_beats(&m, b.len());
        for (x, y) in g.iter().zip(&b) {
            assert!((x - y).abs() < 0.04, "{x} {y}");
        }
    }

    fn skeleton(beats: Vec<f64>, downbeats: Vec<f64>) -> Skeleton {
        Skeleton {
            duration_sec: beats.last().copied().unwrap_or(0.0) + 1.0,
            tempo_map: tempo_map(&beats),
            beats,
            downbeats,
            bpm: Some(120.0),
            beats_per_bar: Some(4),
            tuning_cents: 0.0,
            key: None,
            chords: vec![],
            groove: None,
        }
    }

    #[test]
    fn grid_puts_the_first_downbeat_on_a_bar_and_the_audio_inside_the_song() {
        // 120 BPM。音声の 1.3 秒から拍、3 拍目(2.3 秒)が最初の小節頭(弱起 2 拍)
        let beats: Vec<f64> = (0..40).map(|i| 1.3 + i as f64 * 0.5).collect();
        let sk = skeleton(beats.clone(), vec![2.3, 4.3]);
        let plan = grid_plan(&sk, &GridOptions::default()).unwrap();
        let map = plan.tempo_map();
        // 最初の小節頭が小節の頭(tick が 1 小節の倍数)
        let t = plan.tick_of(&map, 2.3);
        assert!((t / 3840.0 - (t / 3840.0).round()).abs() < 0.01, "{t}");
        // 弱起の小節(0.3 秒から)より前は切る(前奏を残すなら小節を足して、音声の頭が曲の 0 秒以降に入る)
        assert!(
            (plan.audio_start_sec + 0.3).abs() < 0.02,
            "{}",
            plan.audio_start_sec
        );
        let keep = grid_plan(
            &sk,
            &GridOptions {
                keep_intro: Some(true),
                ..Default::default()
            },
        )
        .unwrap();
        assert!(keep.audio_start_sec >= 0.0, "{}", keep.audio_start_sec);
        // 拍はどれも拍の格子に乗る
        for b in &beats {
            let t = plan.tick_of(&map, *b);
            assert!((t / 960.0 - (t / 960.0).round()).abs() < 0.03, "{b} → {t}");
        }
        // 弱起の小節(1 小節目)は最初の小節頭の 1 小節前から。番号は 1 から
        let bars = grid_bars(&plan);
        assert_eq!(bars[0].0, 1);
        assert!((bars[0].1 - 0.3).abs() < 0.02, "{bars:?}");
        assert!((bars[1].1 - 2.3).abs() < 0.02, "{bars:?}");
        // 弱起なし: 最初の小節頭(2.3 秒)が曲の頭(1 小節目)。その前は切る
        let cut = grid_plan(
            &sk,
            &GridOptions {
                no_pickup: Some(true),
                ..Default::default()
            },
        )
        .unwrap();
        assert!(
            (cut.audio_start_sec + 2.3).abs() < 0.02,
            "{}",
            cut.audio_start_sec
        );
        assert!(cut.tick_of(&cut.tempo_map(), 2.3).abs() < 20.0);
        let bars = grid_bars(&cut);
        assert_eq!(bars[0].0, 1);
        assert!((bars[0].1 - 2.3).abs() < 0.02, "{bars:?}");
        // 小節頭を 1 拍ずらすと、2.8 秒が小節の頭
        let plan = grid_plan(
            &sk,
            &GridOptions {
                downbeat_shift: 1,
                ..Default::default()
            },
        )
        .unwrap();
        let t = plan.tick_of(&plan.tempo_map(), 2.8);
        assert!((t / 3840.0 - (t / 3840.0).round()).abs() < 0.01, "{t}");
        // 倍の速さに取り直すと 240 BPM
        let plan = grid_plan(
            &sk,
            &GridOptions {
                tempo_factor: 2.0,
                ..Default::default()
            },
        )
        .unwrap();
        assert!((plan.tempo[0].bpm - 240.0).abs() < 1.0, "{:?}", plan.tempo);
    }

    #[test]
    fn notes_become_a_track_on_the_grid() {
        let beats: Vec<f64> = (0..40).map(|i| i as f64 * 0.5).collect();
        let sk = skeleton(beats, vec![0.0, 2.0]);
        let plan = grid_plan(&sk, &GridOptions::default()).unwrap();
        let notes = vec![
            RawNote {
                start: 0.02,
                end: 0.49,
                pitch: 60,
                vel: 90,
            },
            RawNote {
                start: 0.51,
                end: 0.74,
                pitch: 62,
                vel: 90,
            },
        ];
        let part = PartSpec {
            name: "Melody".into(),
            kind: PartKind::Melody,
            program: Some(0),
        };
        let cmds = part_track_commands(&plan, &part, &notes, 1.0, Some("GeneralUser-GS.sf2"));
        let glaux_core::Command::AddClip { clip, .. } = &cmds[1] else {
            panic!("{cmds:?}");
        };
        let n = clip.notes().unwrap();
        // 1 拍 = 960 tick。0.02〜0.49 秒は 1 拍目の 1 拍分、0.51〜0.74 秒は 2 拍目の 8 分音符
        assert_eq!((n[0].pos.0, n[0].dur.0), (0, 960));
        assert_eq!((n[1].pos.0, n[1].dur.0), (960, 480));
    }

    #[test]
    fn reference_times_map_to_song_beats() {
        let beats: Vec<f64> = (0..40).map(|i| 1.3 + i as f64 * 0.5).collect();
        let sk = skeleton(beats, vec![2.3, 4.3]);
        // 弱起なし: 2.3 秒(最初の小節頭)が 0 拍、2.55 秒は 0.5 拍
        let plan = grid_plan(
            &sk,
            &GridOptions {
                no_pickup: Some(true),
                ..Default::default()
            },
        )
        .unwrap();
        assert!(song_beat(&plan, 2.3).abs() < 1e-9);
        assert!((song_beat(&plan, 2.55) - 0.5).abs() < 1e-9);
        assert!((song_beat(&plan, 6.3) - 8.0).abs() < 1e-9);
        // 前の拍の外側(1.3 秒より前)は最初の拍の長さで延ばす
        assert!((song_beat(&plan, 1.05) - (-2.5)).abs() < 1e-9);
    }

    #[test]
    fn transpose_follows_the_song_key_and_mode() {
        use glaux_core::{
            Clip, ClipContent, ClipId, Note, NoteId, Tick, Track, TrackId, TrackKind,
        };
        // 今の曲: G 長調の音階
        let mut p = glaux_core::Project::new("t");
        let mut t = Track::new(TrackId::new(), "m", TrackKind::Midi);
        let mut c = Clip::new_midi(ClipId::new(), "c", Tick(0), Tick(3840 * 4));
        if let ClipContent::Midi { notes, .. } = &mut c.content {
            for (i, pitch) in [
                55u8, 59, 62, 67, 66, 64, 62, 60, 59, 57, 55, 62, 67, 71, 74, 79,
            ]
            .iter()
            .enumerate()
            {
                notes.push(Note {
                    locked: false,
                    id: NoteId::new(),
                    pos: Tick(i as u64 * 960),
                    dur: Tick(900),
                    pitch: *pitch,
                    vel: 90,
                    articulation: Default::default(),
                    pitch_curve: vec![],
                    glide_ms: None,
                    vibrato: None,
                    volume_curve: vec![],
                    brightness_curve: vec![],
                    condition: None,
                });
            }
        }
        t.clips.push(c);
        p.tracks.push(t);
        let mut sk = skeleton((0..10).map(|i| i as f64 * 0.5).collect(), vec![0.0]);
        // 参考曲が C 長調 → 7 半音上(G)ではなく 5 半音下
        sk.key = Some(KeyGuess {
            tonic: 0,
            minor: false,
            name: "C major".into(),
            confidence: 1.0,
        });
        assert_eq!(auto_transpose(&sk, &p), -5);
        // 参考曲が A 短調 → G 長調の平行調(E 短調)へ: 5 半音下
        sk.key = Some(KeyGuess {
            tonic: 9,
            minor: true,
            name: "A minor".into(),
            confidence: 1.0,
        });
        assert_eq!(auto_transpose(&sk, &p), -5);
        // 今の曲に音が無ければそのまま
        assert_eq!(auto_transpose(&sk, &glaux_core::Project::new("e")), 0);
    }

    #[test]
    fn segments_cover_beats() {
        let (bounds, pos) = beat_segments(&[1.0, 1.5, 2.0, 2.5, 3.0], &[1.0, 3.0], 4.0);
        assert_eq!(bounds.first(), Some(&(0.0, 1.0)));
        assert_eq!(bounds.last(), Some(&(3.5, 4.0)));
        assert_eq!(&pos[1..5], &[Some(0), Some(1), Some(2), Some(3)]);
        assert_eq!(pos[5], Some(0));
    }
}
