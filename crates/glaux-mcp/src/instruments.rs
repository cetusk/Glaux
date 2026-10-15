//! 曲の中の楽器の見分け(曲から起こす機能で、曲に合った楽器のトラックを作るため)。
//!
//! 音を 10 秒の区間(5 秒ずらし)に切り、LAION-CLAP の埋め込み(`glaux_ml::clap`)を求めて、同梱の音色語の辞書の
//! 楽器の語との近さ(z 値: その語が「いろいろな音」に対して出す値と比べた近さ)を測る。種類ごとに、属する語の z 値の最大を点数にする。
//! 学習はしない(言葉で分類する)。CLAP の音声側のモデルは利用者が取得したもの(`glaux_ml::clap::available`)。

use serde::Serialize;

/// 再現する楽器の種類
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, serde::Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum InstrumentClass {
    Drums,
    Bass,
    Pad,
    Strings,
    Piano,
    Epiano,
    Organ,
    /// 短いシンセ(プラック・スタブ・アルペジオ)
    Pluck,
    /// ベル・マリンバなど
    Bell,
    Lead,
    Guitar,
    Brass,
    /// 管(フルートなど)
    Wind,
    /// 歌・声
    Voice,
    /// 効果音(ライザー・ノイズ・機械音など)
    Fx,
}

/// 種類と、その種類に属する辞書の語(英語)・日本語の名前
pub const CLASSES: [(InstrumentClass, &str, &[&str]); 15] = [
    (
        InstrumentClass::Drums,
        "ドラム",
        &[
            "a kick drum",
            "a snare drum",
            "a hi-hat",
            "a cymbal",
            "a hand clap",
            "a tom drum",
            "percussion",
            "a drum loop",
        ],
    ),
    (
        InstrumentClass::Bass,
        "ベース",
        &[
            "a synth bass",
            "a sub bass",
            "an 808 bass",
            "an acid bass TB-303",
            "a bass guitar",
        ],
    ),
    (InstrumentClass::Pad, "パッド", &["a synth pad"]),
    (
        InstrumentClass::Strings,
        "ストリングス",
        &["a string ensemble"],
    ),
    (InstrumentClass::Piano, "ピアノ", &["an acoustic piano"]),
    (InstrumentClass::Epiano, "エレピ", &["an electric piano"]),
    (InstrumentClass::Organ, "オルガン", &["an organ"]),
    (
        InstrumentClass::Pluck,
        "シンセのプラック",
        &["a synth pluck", "an arpeggiated synth"],
    ),
    (
        InstrumentClass::Bell,
        "ベル",
        &["an FM synth bell", "a bell", "a marimba"],
    ),
    (
        InstrumentClass::Lead,
        "シンセリード",
        &[
            "a synth lead",
            "a supersaw synth",
            "chiptune 8-bit video game music",
        ],
    ),
    (
        InstrumentClass::Guitar,
        "ギター",
        &[
            "an acoustic guitar",
            "a clean electric guitar",
            "a distorted electric guitar",
            "a harp",
        ],
    ),
    (InstrumentClass::Brass, "ブラス", &["a brass section"]),
    (InstrumentClass::Wind, "管楽器", &["a flute", "whistling"]),
    (InstrumentClass::Voice, "声", &["a human voice", "a choir"]),
    (
        InstrumentClass::Fx,
        "効果音",
        &["a noise riser", "a sound effect"],
    ),
];

impl InstrumentClass {
    /// 日本語の名前
    pub fn ja(self) -> &'static str {
        CLASSES.iter().find(|c| c.0 == self).map_or("", |c| c.1)
    }
}

/// 区間 1 つの、種類ごとの点数(z 値)
#[derive(Clone, Debug, Serialize)]
pub struct WindowScores {
    pub start: f64,
    pub end: f64,
    pub scores: Vec<(InstrumentClass, f32)>,
}

impl WindowScores {
    pub fn score(&self, c: InstrumentClass) -> f32 {
        self.scores
            .iter()
            .find(|s| s.0 == c)
            .map_or(f32::MIN, |s| s.1)
    }
}

/// 区間の長さと、ずらす幅(秒)
pub const WINDOW_SECS: f64 = 10.0;
pub const HOP_SECS: f64 = 5.0;

/// 区間 1 つの埋め込み(ほぼ無音の区間は None)
#[derive(Clone, Debug, Serialize, serde::Deserialize)]
pub struct WindowEmbedding {
    pub start: f64,
    pub end: f64,
    pub embedding: Option<Vec<f32>>,
}

/// 音(モノラル)を区間に切り、CLAP の埋め込みを求める。曲全体より 30dB 以上小さい区間は無音として扱う
pub fn window_embeddings(frames: &[f32], sr: f32) -> Result<Vec<WindowEmbedding>, String> {
    if !glaux_ml::clap::available() {
        return Err("CLAP の音声モデルがありません(楽器の見分けに使う)".into());
    }
    let rms = |x: &[f32]| -> f32 {
        let e = x.iter().map(|v| v * v).sum::<f32>() / x.len().max(1) as f32;
        10.0 * (e + 1e-12).log10()
    };
    let total = rms(frames);
    let win = (WINDOW_SECS * sr as f64) as usize;
    let hop = (HOP_SECS * sr as f64) as usize;
    let mut out = Vec::new();
    let mut start = 0usize;
    while start < frames.len() {
        let end = (start + win).min(frames.len());
        let seg = &frames[start..end];
        let quiet = seg.len() < (sr as usize) || rms(seg) < total - 30.0;
        let embedding = if quiet {
            None
        } else {
            Some(glaux_ml::clap::embed(seg, sr).map_err(|e| e.to_string())?)
        };
        out.push(WindowEmbedding {
            start: start as f64 / sr as f64,
            end: end as f64 / sr as f64,
            embedding,
        });
        if end == frames.len() {
            break;
        }
        start += hop;
    }
    Ok(out)
}

/// 区間の埋め込みから、種類ごとの点数(音色語の辞書の楽器の語との近さ。z 値)を求める
pub fn word_scores(windows: &[WindowEmbedding]) -> Vec<WindowScores> {
    let words: Vec<(InstrumentClass, Vec<&'static glaux_ml::clap::VocabWord>)> = CLASSES
        .iter()
        .map(|(c, _, ws)| {
            (
                *c,
                ws.iter()
                    .filter_map(|w| glaux_ml::clap::find_word(w))
                    .collect(),
            )
        })
        .collect();
    windows
        .iter()
        .map(|w| WindowScores {
            start: w.start,
            end: w.end,
            scores: words
                .iter()
                .map(|(c, ws)| {
                    let z = match &w.embedding {
                        Some(e) => ws.iter().map(|v| v.z(e)).fold(f32::MIN, f32::max),
                        None => f32::MIN,
                    };
                    (*c, z)
                })
                .collect(),
        })
        .collect()
}

/// 楽器の見本(ある種類の音色で鳴らしたフレーズの埋め込み)
#[derive(Clone, Debug, Serialize, serde::Deserialize)]
pub struct Prototype {
    pub class: InstrumentClass,
    pub embedding: Vec<f32>,
}

/// 区間の埋め込みから、種類ごとの点数(その種類の見本のうち近い 2 つとのコサイン類似度の平均)を求める
pub fn prototype_scores(windows: &[WindowEmbedding], protos: &[Prototype]) -> Vec<WindowScores> {
    windows
        .iter()
        .map(|w| WindowScores {
            start: w.start,
            end: w.end,
            scores: CLASSES
                .iter()
                .map(|(c, _, _)| {
                    let s = match &w.embedding {
                        Some(e) => {
                            let mut v: Vec<f32> = protos
                                .iter()
                                .filter(|p| p.class == *c)
                                .map(|p| glaux_ml::clap::similarity(e, &p.embedding))
                                .collect();
                            v.sort_by(|a, b| b.total_cmp(a));
                            if v.is_empty() {
                                f32::MIN
                            } else {
                                v.iter().take(2).sum::<f32>() / v.len().min(2) as f32
                            }
                        }
                        None => f32::MIN,
                    };
                    (*c, s)
                })
                .collect(),
        })
        .collect()
}
