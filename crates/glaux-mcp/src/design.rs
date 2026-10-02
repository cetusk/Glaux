//! 文章から音色を作る(MCP の design_sound)。「暗くて太いベース、少し揺れる」から内蔵音源のパッチを作る。
//!
//! 二段構え:
//! 1. **対応表**([`LEXICON`]): 言葉 → 役割(ベース・パッド…)と、共通の大きなつまみ(明るさ・太さ…、[`crate::character`])の
//!    動かす向きと量。「少し・とても」の強さ、「〜なし・〜ない」の否定、日本語の感覚語の同義語も表で受ける。
//!    決まった結果になり、どの言葉でどのつまみを動かしたかを説明でき、1 回で大まかな音ができる
//! 2. **CLAP の採点**(モデルがあるとき): 作った音を描き出し、言葉に対応する音色語(英語)との近さを測って、
//!    つまみを 10〜20 回だけ追い込む(良くならなければ 1 段目のまま)
//!
//! 結果は普通に編集できるパッチ(音源 + エフェクト)。マクロは作らない(後から set_character で動かせる)。

use crate::character::{feel_targets, FEELS};
use glaux_core::{Device, Effect, FxId, ParamPath, ParamValue, Track, TrackId, TrackKind};

/// 役割(土台の音色)
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    Bass,
    Lead,
    Pad,
    Pluck,
    Keys,
    Bell,
    Organ,
    Strings,
    Brass,
    Supersaw,
    Wobble,
    Vocal,
}

impl Role {
    pub fn name(self) -> &'static str {
        match self {
            Role::Bass => "ベース",
            Role::Lead => "リード",
            Role::Pad => "パッド",
            Role::Pluck => "プラック",
            Role::Keys => "エレピ",
            Role::Bell => "ベル",
            Role::Organ => "オルガン",
            Role::Strings => "ストリングス",
            Role::Brass => "ブラス",
            Role::Supersaw => "スーパーソー",
            Role::Wobble => "ウォブルベース",
            Role::Vocal => "母音のシンセ",
        }
    }
}

/// 言葉が足す特別なもの(つまみの向き以外)
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Extra {
    /// テープの揺れと飽和(ローファイ・レトロ)
    Lofi,
    /// 矩形波 + ビットクラッシュ(ゲーム機)
    Chip,
    /// 雑音を混ぜる(息っぽい)
    Breathy,
    /// 空間のエフェクトを外す(「リバーブなし」)
    NoSpace,
}

/// 対応表の 1 項目
pub struct Lex {
    pub words: &'static [&'static str],
    /// (つまみのキー, 動かす量 −0.5〜0.5)
    pub feels: &'static [(&'static str, f64)],
    pub role: Option<Role>,
    pub extra: Option<Extra>,
    /// CLAP の音色語(英語。採点に使う)
    pub vocab: &'static [&'static str],
    /// 説明(返り値に使う)
    pub meaning: &'static str,
}

macro_rules! lex {
    ($words:expr, $feels:expr, $role:expr, $extra:expr, $vocab:expr, $meaning:expr) => {
        Lex {
            words: $words,
            feels: $feels,
            role: $role,
            extra: $extra,
            vocab: $vocab,
            meaning: $meaning,
        }
    };
}

/// 言葉の対応表。日本語は語幹(「明る」で「明るい・明るく」の両方)。長い語から照合する
pub const LEXICON: &[Lex] = &[
    // ---- 役割 ----
    lex!(
        &["ベース", "bass", "低音", "サブ", "sub", "808"],
        &[],
        Some(Role::Bass),
        None,
        &[],
        "ベース(低音の支え)"
    ),
    lex!(
        &["リード", "lead", "メロディ", "主旋律", "ソロ"],
        &[],
        Some(Role::Lead),
        None,
        &[],
        "リード(主旋律)"
    ),
    lex!(
        &["パッド", "pad", "背景", "持続音", "ドローン", "drone"],
        &[],
        Some(Role::Pad),
        None,
        &[],
        "パッド(伸ばしの和音)"
    ),
    lex!(
        &["プラック", "pluck", "ポロン", "はじい", "アルペジオ", "arp"],
        &[],
        Some(Role::Pluck),
        None,
        &["plucked"],
        "プラック(はじいた短い音)"
    ),
    lex!(
        &[
            "エレピ",
            "ローズ",
            "rhodes",
            "electric piano",
            "鍵盤",
            "keys",
            "ピアノ"
        ],
        &[],
        Some(Role::Keys),
        None,
        &[],
        "エレピ(鍵盤)"
    ),
    lex!(
        &["ベル", "bell", "鐘", "チャイム", "chime", "オルゴール"],
        &[],
        Some(Role::Bell),
        None,
        &["metallic"],
        "ベル(鐘・チャイム)"
    ),
    lex!(
        &["オルガン", "organ"],
        &[],
        Some(Role::Organ),
        None,
        &[],
        "オルガン"
    ),
    lex!(
        &["ストリングス", "strings", "弦楽", "オーケストラ"],
        &[],
        Some(Role::Strings),
        None,
        &[],
        "ストリングス(弦の合奏風)"
    ),
    lex!(
        &["ブラス", "brass", "ホーン", "horn", "金管"],
        &[],
        Some(Role::Brass),
        None,
        &[],
        "ブラス(金管風)"
    ),
    lex!(
        &["スーパーソー", "supersaw", "トランス", "trance"],
        &[],
        Some(Role::Supersaw),
        None,
        &["detuned"],
        "スーパーソー"
    ),
    lex!(
        &["ウォブル", "ワブル", "wobble", "ダブステップ", "dubstep"],
        &[("motion", 0.3)],
        Some(Role::Wobble),
        None,
        &["wobbling"],
        "ウォブル(うねる低音)"
    ),
    lex!(
        &[
            "母音",
            "ボーカルっぽ",
            "声のよう",
            "vocal",
            "クワイア",
            "choir"
        ],
        &[],
        Some(Role::Vocal),
        None,
        &[],
        "母音のような音"
    ),
    // ---- 明るさ ----
    lex!(
        &[
            "明る",
            "ブライト",
            "bright",
            "きらきら",
            "キラキラ",
            "きらびやか",
            "抜けの良",
            "シャキ"
        ],
        &[("brightness", 0.25)],
        None,
        None,
        &["bright"],
        "明るい"
    ),
    lex!(
        &["クリスプ", "crisp", "エアリー", "airy", "空気感"],
        &[("brightness", 0.2)],
        None,
        None,
        &["crisp", "airy"],
        "抜けが良い・空気感"
    ),
    lex!(
        &["暗", "ダーク", "dark"],
        &[("brightness", -0.25)],
        None,
        None,
        &["dark"],
        "暗い"
    ),
    lex!(
        &["こもっ", "こもり", "籠", "muffled", "くぐもっ"],
        &[("brightness", -0.3)],
        None,
        None,
        &["muffled"],
        "こもった"
    ),
    lex!(
        &["丸い", "まるい", "まろやか", "mellow", "マイルド", "mild"],
        &[("brightness", -0.15), ("attack", -0.1)],
        None,
        None,
        &["mellow"],
        "まろやか"
    ),
    // ---- 太さ ----
    lex!(
        &[
            "太",
            "ファット",
            "fat",
            "thick",
            "分厚",
            "厚み",
            "重",
            "heavy",
            "どっしり",
            "ずっしり",
            "パワフル",
            "powerful",
            "迫力"
        ],
        &[("body", 0.25)],
        None,
        None,
        &["thick and fat"],
        "太い・厚い"
    ),
    lex!(
        &["細", "薄", "ペラ", "thin", "軽い", "軽め"],
        &[("body", -0.25)],
        None,
        None,
        &["thin"],
        "細い・軽い"
    ),
    lex!(
        &["豊か", "リッチ", "rich"],
        &[("body", 0.15), ("width", 0.1)],
        None,
        None,
        &["rich"],
        "豊か"
    ),
    // ---- 動き ----
    lex!(
        &[
            "揺れ",
            "揺ら",
            "ゆらゆら",
            "うね",
            "動き",
            "動く",
            "生きた",
            "wobbly",
            "moving",
            "evolving",
            "変化する"
        ],
        &[("motion", 0.25)],
        None,
        None,
        &["evolving"],
        "揺れる・動く"
    ),
    lex!(
        &[
            "アナログ",
            "analog",
            "ヴィンテージ",
            "ビンテージ",
            "vintage"
        ],
        &[("motion", 0.2), ("grit", 0.05)],
        None,
        None,
        &["analog", "vintage"],
        "アナログ・ヴィンテージ"
    ),
    lex!(
        &[
            "止まった",
            "静的",
            "static",
            "デジタル",
            "digital",
            "正確",
            "機械的"
        ],
        &[("motion", -0.2)],
        None,
        None,
        &["digital", "static"],
        "止まった・デジタル"
    ),
    // ---- 広がり ----
    lex!(
        &[
            "広い",
            "広が",
            "ワイド",
            "wide",
            "ステレオ",
            "stereo",
            "包み込",
            "壮大",
            "epic"
        ],
        &[("width", 0.3)],
        None,
        None,
        &["spacious"],
        "広い"
    ),
    lex!(
        &["狭", "モノ", "mono", "中央", "センター", "タイト", "tight"],
        &[("width", -0.3)],
        None,
        None,
        &["short and tight"],
        "狭い・中央"
    ),
    // ---- 空間 ----
    lex!(
        &[
            "遠",
            "空間",
            "響",
            "リバーブ",
            "reverb",
            "残響",
            "ホール",
            "hall",
            "アンビエント",
            "ambient",
            "奥行",
            "深い"
        ],
        &[("space", 0.3)],
        None,
        None,
        &["reverberant", "distant"],
        "響く・遠い"
    ),
    lex!(
        &[
            "近",
            "ドライ",
            "dry",
            "乾い",
            "デッド",
            "dead",
            "親密",
            "intimate"
        ],
        &[("space", -0.35)],
        None,
        None,
        &["dry", "close and intimate"],
        "近い・乾いた"
    ),
    lex!(
        &["夢", "dreamy", "幻想", "ethereal", "浮遊"],
        &[("space", 0.25), ("attack", -0.15), ("motion", 0.15)],
        None,
        None,
        &["dreamy", "ethereal"],
        "夢のような"
    ),
    // ---- アタック ----
    lex!(
        &[
            "鋭",
            "シャープ",
            "sharp",
            "パンチ",
            "punch",
            "歯切れ",
            "アタック",
            "硬",
            "hard",
            "はっきり",
            "キレ",
            "打撃",
            "percussive"
        ],
        &[("attack", 0.3)],
        None,
        None,
        &["punchy", "percussive"],
        "鋭い・パンチ"
    ),
    lex!(
        &[
            "柔らか",
            "やわらか",
            "ソフト",
            "soft",
            "ふわ",
            "ふんわり",
            "優し",
            "やさし",
            "gentle",
            "なめらか",
            "smooth"
        ],
        &[("attack", -0.3), ("brightness", -0.05)],
        None,
        None,
        &["soft", "smooth"],
        "柔らかい"
    ),
    lex!(
        &["ゆっくり立ち上が", "スウェル", "swell", "じわっと"],
        &[("attack", -0.45)],
        None,
        None,
        &["slowly swelling"],
        "ゆっくり立ち上がる"
    ),
    // ---- 歪み ----
    lex!(
        &[
            "歪",
            "ディストーション",
            "distort",
            "ざら",
            "ザラ",
            "gritty",
            "汚",
            "dirty",
            "荒",
            "crunch",
            "ファズ",
            "fuzz",
            "ノイジー",
            "noisy"
        ],
        &[("grit", 0.35)],
        None,
        None,
        &["distorted", "gritty"],
        "歪んだ・ざらついた"
    ),
    lex!(
        &["攻撃的", "aggressive", "激しい", "アグレッシブ"],
        &[("grit", 0.3), ("attack", 0.2), ("brightness", 0.1)],
        None,
        None,
        &["aggressive"],
        "攻撃的"
    ),
    lex!(
        &[
            "クリーン",
            "clean",
            "綺麗",
            "きれい",
            "澄ん",
            "pure",
            "透明"
        ],
        &[("grit", -0.3)],
        None,
        None,
        &["clean"],
        "きれい・澄んだ"
    ),
    // ---- 複合 ----
    lex!(
        &["温か", "暖か", "あたたか", "ウォーム", "warm"],
        &[("brightness", -0.1), ("body", 0.15), ("grit", 0.1)],
        None,
        None,
        &["warm"],
        "温かい"
    ),
    lex!(
        &["冷た", "クール", "cold", "cool", "無機質"],
        &[("brightness", 0.1), ("motion", -0.15)],
        None,
        None,
        &["cold"],
        "冷たい"
    ),
    lex!(
        &["金属", "メタリック", "metallic"],
        &[("brightness", 0.1)],
        Some(Role::Bell),
        None,
        &["metallic"],
        "金属的"
    ),
    lex!(
        &["ガラス", "glassy", "透き通"],
        &[("brightness", 0.15)],
        Some(Role::Bell),
        None,
        &["glassy"],
        "ガラスのような"
    ),
    lex!(
        &[
            "ローファイ",
            "lo-fi",
            "lofi",
            "レトロ",
            "retro",
            "懐かし",
            "古い"
        ],
        &[("brightness", -0.15)],
        None,
        Some(Extra::Lofi),
        &["lo-fi", "vintage"],
        "ローファイ・レトロ"
    ),
    lex!(
        &[
            "8bit",
            "8ビット",
            "8-bit",
            "ピコピコ",
            "チップチューン",
            "chiptune",
            "ゲーム機",
            "ファミコン"
        ],
        &[("motion", -0.2)],
        None,
        Some(Extra::Chip),
        &["digital", "buzzy"],
        "ゲーム機(8bit)"
    ),
    lex!(
        &["息", "breathy", "吐息"],
        &[("brightness", 0.05)],
        None,
        Some(Extra::Breathy),
        &["breathy"],
        "息っぽい"
    ),
];

/// 強めの語(その語の直前に付く)
const STRONG: &[&str] = &[
    "とても",
    "すごく",
    "かなり",
    "超",
    "めちゃ",
    "very",
    "really",
    "super",
    "extremely",
    "思い切り",
    "思いっきり",
];
/// 弱めの語
const WEAK: &[&str] = &[
    "少し",
    "ちょっと",
    "やや",
    "軽く",
    "ほんのり",
    "slightly",
    "a bit",
    "a little",
    "somewhat",
    "わずかに",
];
/// 否定(その語の直後)
const NEG_AFTER: &[&str] = &[
    "ない",
    "なく",
    "無し",
    "なし",
    "じゃな",
    "ではな",
    "控えめ",
    "less",
    "すぎない",
    "過ぎない",
    "くない",
    "くな",
    "しない",
];
/// 否定(その語の直前)
const NEG_BEFORE: &[&str] = &["no ", "not ", "without ", "non-", "非", "脱"];

/// 照合した言葉 1 つ
#[derive(Clone, Debug, serde::Serialize)]
pub struct Matched {
    pub word: String,
    pub meaning: &'static str,
    /// 強さ(1 = ふつう、0.5 = 少し、1.6 = とても、負 = 否定)
    pub strength: f64,
}

/// 文から読み取ったこと
#[derive(Clone, Debug)]
pub struct Intent {
    pub role: Role,
    /// つまみの動かす量(FEELS の順。−0.5〜0.5)
    pub feels: [f64; 7],
    pub extras: Vec<Extra>,
    /// 近づけたい・遠ざけたい音色語(CLAP)
    pub toward: Vec<&'static str>,
    pub away: Vec<&'static str>,
    pub matched: Vec<Matched>,
}

fn feel_index(key: &str) -> usize {
    FEELS.iter().position(|f| f.0 == key).unwrap_or(0)
}

/// 文を読む
pub fn read(text: &str) -> Intent {
    let lower = text.to_lowercase();
    // (位置, 長さ, 項目, 語)を、長い語から、重ならないように集める
    let mut words: Vec<(&'static str, usize)> = LEXICON
        .iter()
        .enumerate()
        .flat_map(|(i, l)| l.words.iter().map(move |w| (*w, i)))
        .collect();
    words.sort_by_key(|(w, _)| std::cmp::Reverse(w.chars().count()));
    let mut taken: Vec<(usize, usize)> = Vec::new();
    let mut hits: Vec<(usize, usize, usize, &'static str)> = Vec::new();
    for (w, li) in words {
        let wl = w.to_lowercase();
        let mut from = 0;
        while let Some(off) = lower[from..].find(&wl) {
            let p = from + off;
            let e = p + wl.len();
            if !taken.iter().any(|(a, b)| p < *b && e > *a) {
                taken.push((p, e));
                hits.push((p, e, li, w));
            }
            from = e;
        }
    }
    hits.sort_by_key(|h| h.0);
    let mut intent = Intent {
        role: Role::Lead,
        feels: [0.0; 7],
        extras: Vec::new(),
        toward: Vec::new(),
        away: Vec::new(),
        matched: Vec::new(),
    };
    let mut role_set = false;
    for (p, e, li, w) in hits {
        let l = &LEXICON[li];
        // 前後の窓(文字で数える)
        let before: String = {
            let s: Vec<char> = lower[..p].chars().collect();
            s[s.len().saturating_sub(8)..].iter().collect()
        };
        let after: String = lower[e..].chars().take(5).collect();
        // 強弱の語は直前に付くときだけ(「とても暗くて太い」の「太い」は強めない)
        let near = before.trim_end_matches([' ', '、', ',']);
        let mut strength: f64 = 1.0;
        if STRONG.iter().any(|k| near.ends_with(k)) {
            strength = 1.6;
        }
        if WEAK.iter().any(|k| near.ends_with(k)) {
            strength = 0.5;
        }
        let negated = NEG_AFTER.iter().any(|k| after.starts_with(k))
            || NEG_BEFORE.iter().any(|k| before.ends_with(k));
        if negated {
            strength = -strength.abs();
        }
        if let Some(r) = l.role {
            // 役割は最初に出たもの(否定されていなければ)
            if !role_set && !negated {
                intent.role = r;
                role_set = true;
            }
        }
        for (k, d) in l.feels {
            let i = feel_index(k);
            intent.feels[i] = (intent.feels[i] + d * strength).clamp(-0.5, 0.5);
        }
        if let (Some(Extra::Lofi | Extra::Chip | Extra::Breathy), false) = (l.extra, negated) {
            intent.extras.extend(l.extra);
        }
        // 「リバーブなし」「響かない」: 空間を外す
        if negated && l.feels.iter().any(|(k, _)| *k == "space") {
            intent.extras.push(Extra::NoSpace);
        }
        for v in l.vocab {
            if negated {
                intent.away.push(v);
            } else {
                intent.toward.push(v);
            }
        }
        intent.matched.push(Matched {
            word: w.to_owned(),
            meaning: l.meaning,
            strength,
        });
    }
    intent.extras.dedup();
    intent.toward.dedup();
    intent.away.dedup();
    intent
}

fn fx(name: &str, params: &[(&str, f64)]) -> Effect {
    let mut e = Effect::builtin(FxId::new(), name);
    for (k, v) in params {
        e.params.insert((*k).to_owned(), ParamValue::Float(*v));
    }
    e
}

fn dev(name: &str, params: &[(&str, f64)]) -> Device {
    let mut d = Device::builtin(name);
    for (k, v) in params {
        d.params.insert((*k).to_owned(), ParamValue::Float(*v));
    }
    d
}

fn set_enum(d: &mut Device, k: &str, v: &str) {
    d.params
        .insert(k.to_owned(), ParamValue::Enum(v.to_owned()));
}

/// 役割ごとの土台の音色(音源, エフェクト)
pub fn base(role: Role) -> (Device, Vec<Effect>) {
    match role {
        Role::Bass => {
            let mut d = dev(
                "subtractive",
                &[
                    ("cutoff", 600.0),
                    ("resonance", 0.2),
                    ("drive", 0.2),
                    ("filter_env", 0.45),
                    ("filter_decay", 0.25),
                    ("filter_sustain", 0.15),
                    ("sub", 0.4),
                    ("key_track", 0.3),
                    ("vel_cutoff", 0.3),
                    ("sustain", 0.9),
                    ("release", 0.1),
                    ("gain_db", -10.0),
                ],
            );
            set_enum(&mut d, "filter_type", "lp24");
            (d, vec![])
        }
        Role::Lead => (
            dev(
                "subtractive",
                &[
                    ("unison", 3.0),
                    ("detune", 12.0),
                    ("spread", 0.3),
                    ("analog", 0.15),
                    ("cutoff", 4000.0),
                    ("filter_env", 0.2),
                    ("key_track", 0.4),
                    ("vel_cutoff", 0.3),
                    ("sustain", 0.85),
                    ("release", 0.15),
                    ("gain_db", -13.0),
                ],
            ),
            vec![fx(
                "delay",
                &[("time_ms", 375.0), ("feedback", 0.25), ("mix", 0.12)],
            )],
        ),
        Role::Pad => (
            dev(
                "subtractive",
                &[
                    ("unison", 5.0),
                    ("detune", 18.0),
                    ("spread", 0.7),
                    ("analog", 0.3),
                    ("cutoff", 1800.0),
                    ("filter_env", 0.1),
                    ("key_track", 0.4),
                    ("lfo1_rate", 0.2),
                    ("lfo1_depth", 0.12),
                    ("attack", 0.6),
                    ("sustain", 0.9),
                    ("release", 1.2),
                    ("gain_db", -15.0),
                ],
            ),
            vec![fx("reverb", &[("mix", 0.3), ("size", 0.75)])],
        ),
        Role::Pluck => (
            dev(
                "subtractive",
                &[
                    ("unison", 3.0),
                    ("detune", 14.0),
                    ("spread", 0.5),
                    ("analog", 0.2),
                    ("cutoff", 450.0),
                    ("filter_env", 0.85),
                    ("filter_decay", 0.16),
                    ("vel_cutoff", 0.5),
                    ("key_track", 0.5),
                    ("decay", 0.6),
                    ("sustain", 0.25),
                    ("release", 0.25),
                    ("gain_db", -13.0),
                ],
            ),
            vec![fx("reverb", &[("mix", 0.22), ("size", 0.55)])],
        ),
        Role::Keys => (
            dev(
                "fm",
                &[
                    ("ratio", 1.0),
                    ("index", 2.0),
                    ("index_decay", 0.6),
                    ("index_sustain", 0.15),
                    ("decay", 2.5),
                    ("sustain", 0.3),
                    ("release", 0.4),
                    ("gain_db", -10.0),
                ],
            ),
            vec![
                fx("chorus", &[("mix", 0.3)]),
                fx("reverb", &[("mix", 0.18), ("size", 0.5)]),
            ],
        ),
        Role::Bell => (
            dev(
                "fm",
                &[
                    ("ratio", 3.5),
                    ("index", 3.0),
                    ("index_decay", 0.4),
                    ("index_sustain", 0.1),
                    ("decay", 1.5),
                    ("sustain", 0.0),
                    ("release", 0.8),
                    ("gain_db", -12.0),
                ],
            ),
            vec![fx("reverb", &[("mix", 0.3), ("size", 0.7)])],
        ),
        Role::Organ => {
            let mut d = dev(
                "wavetable",
                &[
                    ("position", 0.3),
                    ("cutoff", 6000.0),
                    ("attack", 0.01),
                    ("sustain", 1.0),
                    ("release", 0.08),
                    ("gain_db", -12.0),
                ],
            );
            set_enum(&mut d, "table", "organ");
            (d, vec![fx("chorus", &[("mix", 0.2)])])
        }
        Role::Strings => (
            dev(
                "subtractive",
                &[
                    ("unison", 4.0),
                    ("detune", 10.0),
                    ("spread", 0.6),
                    ("analog", 0.25),
                    ("cutoff", 2500.0),
                    ("key_track", 0.5),
                    ("lfo1_rate", 5.0),
                    ("lfo1_depth", 0.03),
                    ("attack", 0.35),
                    ("sustain", 0.9),
                    ("release", 0.6),
                    ("gain_db", -14.0),
                ],
            ),
            vec![fx("reverb", &[("mix", 0.28), ("size", 0.7)])],
        ),
        Role::Brass => (
            dev(
                "subtractive",
                &[
                    ("unison", 2.0),
                    ("detune", 8.0),
                    ("cutoff", 900.0),
                    ("filter_env", 0.7),
                    ("filter_attack", 0.06),
                    ("filter_decay", 0.5),
                    ("filter_sustain", 0.45),
                    ("vel_cutoff", 0.5),
                    ("attack", 0.04),
                    ("sustain", 0.85),
                    ("release", 0.15),
                    ("gain_db", -12.0),
                ],
            ),
            vec![fx("reverb", &[("mix", 0.15), ("size", 0.5)])],
        ),
        Role::Supersaw => (
            dev(
                "subtractive",
                &[
                    ("unison", 7.0),
                    ("detune", 30.0),
                    ("spread", 0.8),
                    ("analog", 0.3),
                    ("cutoff", 6000.0),
                    ("filter_env", 0.1),
                    ("key_track", 0.4),
                    ("sustain", 0.85),
                    ("release", 0.35),
                    ("gain_db", -15.0),
                ],
            ),
            vec![
                fx(
                    "delay",
                    &[
                        ("time_ms", 375.0),
                        ("feedback", 0.3),
                        ("mix", 0.15),
                        ("ping_pong", 1.0),
                    ],
                ),
                fx("reverb", &[("mix", 0.2), ("size", 0.7)]),
            ],
        ),
        Role::Wobble => {
            let mut d = dev(
                "wavetable",
                &[
                    ("position", 0.3),
                    ("unison", 3.0),
                    ("detune", 10.0),
                    ("cutoff", 3000.0),
                    ("drive", 0.3),
                    ("lfo1_rate", 4.0),
                    ("lfo1_depth", 0.6),
                    ("sustain", 1.0),
                    ("release", 0.1),
                    ("gain_db", -11.0),
                ],
            );
            set_enum(&mut d, "table", "sync");
            set_enum(&mut d, "lfo1_target", "position");
            (d, vec![])
        }
        Role::Vocal => {
            let mut d = dev(
                "wavetable",
                &[
                    ("position", 0.4),
                    ("unison", 3.0),
                    ("detune", 12.0),
                    ("spread", 0.5),
                    ("lfo1_rate", 0.3),
                    ("lfo1_depth", 0.4),
                    ("attack", 0.2),
                    ("sustain", 0.9),
                    ("release", 0.6),
                    ("gain_db", -13.0),
                ],
            );
            set_enum(&mut d, "table", "vocal");
            set_enum(&mut d, "lfo1_target", "position");
            (d, vec![fx("reverb", &[("mix", 0.25), ("size", 0.6)])])
        }
    }
}

/// 試聴の音(役割ごと): (tick の位置, 長さ, 音, 強さ)
pub fn phrase(role: Role) -> Vec<(u64, u64, u8, u8)> {
    match role {
        Role::Bass | Role::Wobble => vec![
            (0, 960, 36, 110),
            (960, 480, 36, 90),
            (1440, 480, 43, 100),
            (1920, 1920, 41, 110),
        ],
        Role::Pad | Role::Strings | Role::Organ | Role::Vocal => {
            vec![(0, 3840, 57, 90), (0, 3840, 60, 90), (0, 3840, 64, 90)]
        }
        Role::Pluck | Role::Keys | Role::Bell => (0..8)
            .map(|k| {
                (
                    k * 480,
                    400,
                    [60u8, 64, 67, 72, 67, 64, 60, 67][k as usize],
                    [110u8, 80, 95, 70][k as usize % 4],
                )
            })
            .collect(),
        Role::Lead | Role::Brass | Role::Supersaw => {
            vec![
                (0, 960, 72, 110),
                (960, 480, 74, 90),
                (1440, 480, 76, 100),
                (1920, 1920, 79, 110),
            ]
        }
    }
}

/// 読み取ったことからパッチを作る。`offsets` は 2 段目の追い込みで足すつまみの量(FEELS の順)
pub fn patch(intent: &Intent, offsets: &[f64; 7]) -> (Device, Vec<Effect>) {
    let (mut device, mut effects) = base(intent.role);
    for x in &intent.extras {
        match x {
            Extra::Lofi => effects.push(fx(
                "tape",
                &[
                    ("wow", 0.35),
                    ("flutter", 0.3),
                    ("saturation", 0.3),
                    ("tone", 0.4),
                    ("hiss", 0.1),
                ],
            )),
            Extra::Chip => {
                if device.source
                    == (glaux_core::PluginSource::Builtin {
                        name: "subtractive".into(),
                    })
                {
                    set_enum(&mut device, "waveform", "square");
                    for k in ["unison", "spread", "analog"] {
                        device.params.remove(k);
                    }
                }
                effects.push(fx(
                    "bitcrush",
                    &[("bits", 6.0), ("downsample", 4.0), ("mix", 1.0)],
                ));
            }
            Extra::Breathy => {
                if device.source
                    == (glaux_core::PluginSource::Builtin {
                        name: "subtractive".into(),
                    })
                {
                    device.params.insert("noise".into(), ParamValue::Float(0.2));
                    set_enum(&mut device, "noise_color", "pink");
                }
            }
            Extra::NoSpace => effects.retain(|e| {
                !matches!(&e.source, glaux_core::PluginSource::Builtin { name }
                    if name == "reverb" || name == "delay" || name == "convolution")
            }),
        }
    }
    let total: Vec<f64> = (0..7)
        .map(|i| (intent.feels[i] + offsets[i]).clamp(-0.5, 0.5))
        .collect();
    // 空間を足したいのに行き先が無ければ、控えめのリバーブを足す
    let space = total[feel_index("space")];
    let has_space = effects.iter().any(|e| {
        matches!(&e.source, glaux_core::PluginSource::Builtin { name }
            if name == "reverb" || name == "delay" || name == "convolution")
    });
    if space > 0.0 && !has_space {
        effects.push(fx("reverb", &[("mix", 0.15), ("size", 0.6)]));
    }
    // 共通のつまみの割り当て先(土台の値が 50)を、動かす量のぶん回す
    let mut t = Track::new(TrackId::new(), "tmp", TrackKind::Midi);
    t.device = Some(device.clone());
    t.effects = effects.clone();
    for (key, target) in feel_targets(&t, &[]) {
        let d = total[feel_index(key)];
        if d == 0.0 {
            continue;
        }
        let v = target.map((0.5 + d).clamp(0.0, 1.0));
        match &target.target {
            ParamPath::Device { name } => {
                device
                    .params
                    .insert(name.clone(), ParamValue::Float(round(v)));
            }
            ParamPath::Effect { id, name } => {
                if let Some(e) = effects.iter_mut().find(|e| &e.id == id) {
                    e.params.insert(name.clone(), ParamValue::Float(round(v)));
                }
            }
            _ => {}
        }
    }
    (device, effects)
}

fn round(v: f64) -> f64 {
    if v.abs() >= 100.0 {
        v.round()
    } else {
        (v * 1000.0).round() / 1000.0
    }
}

/// 土台からどのつまみがどう変わったか(説明用。音源と、エフェクトの足し引き・つまみ)
pub fn describe_changes(intent: &Intent, device: &Device, effects: &[Effect]) -> Vec<String> {
    let (b, bfx) = base(intent.role);
    let f = |v: Option<&ParamValue>| match v {
        Some(ParamValue::Float(x)) => format!("{}", round(*x)),
        Some(ParamValue::Enum(s)) => s.clone(),
        Some(other) => format!("{other:?}"),
        None => "既定".to_owned(),
    };
    let diff = |prefix: &str,
                a: &glaux_core::ParamMap,
                b: &glaux_core::ParamMap,
                out: &mut Vec<String>| {
        let mut keys: Vec<&String> = a.keys().chain(b.keys()).collect();
        keys.sort();
        keys.dedup();
        for k in keys {
            if a.get(k) != b.get(k) {
                out.push(format!("{prefix}{k}: {} → {}", f(b.get(k)), f(a.get(k))));
            }
        }
    };
    let mut out = Vec::new();
    diff("", &device.params, &b.params, &mut out);
    let name = |e: &Effect| match &e.source {
        glaux_core::PluginSource::Builtin { name } => name.clone(),
        _ => "?".to_owned(),
    };
    for e in effects {
        match bfx.iter().find(|x| name(x) == name(e)) {
            Some(x) => diff(&format!("{}.", name(e)), &e.params, &x.params, &mut out),
            None => out.push(format!("{} を足す", name(e))),
        }
    }
    for x in &bfx {
        if !effects.iter().any(|e| name(e) == name(x)) {
            out.push(format!("{} を外す", name(x)));
        }
    }
    out
}

/// 2 段目の結果
#[derive(Clone, Debug, serde::Serialize)]
pub struct Refined {
    pub offsets: [f64; 7],
    pub score_before: f64,
    pub score_after: f64,
    pub evaluations: usize,
}

/// パッチを描き出す(役割の試聴の音を 120 BPM で。モノ)
pub fn render_patch(
    intent: &Intent,
    device: &Device,
    effects: &[Effect],
) -> Result<Vec<f32>, String> {
    use glaux_core::{Clip, ClipId, Note, NoteId, Project, Tick};
    let mut p = Project::new("試聴");
    let mut t = Track::new(TrackId::new(), "音", TrackKind::Midi);
    t.device = Some(device.clone());
    t.effects = effects.to_vec();
    let mut c = Clip::new_midi(ClipId::new(), "c", Tick(0), Tick(3840 * 2));
    if let Some(ns) = c.notes_mut() {
        for (pos, dur, pitch, vel) in phrase(intent.role) {
            ns.push(Note {
                id: NoteId::new(),
                pos: Tick(pos),
                dur: Tick(dur),
                pitch,
                vel,
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
    let st = glaux_engine::export::render_project_range(
        &p,
        48_000.0,
        &glaux_engine::SampleBank::default(),
        0.0,
        4.0,
    )
    .map_err(|e| e.to_string())?;
    Ok(st
        .as_chunks::<2>()
        .0
        .iter()
        .map(|c| (c[0] + c[1]) * 0.5)
        .collect())
}

/// 2 段目: CLAP で言葉との近さを測りながら、言葉に出たつまみを ±0.25 の中で追い込む(良くならなければ 0 のまま)。
/// モデルが無い・測れる音色語が無い・動かすつまみが無いなら None
pub fn refine(intent: &Intent, max_evals: usize) -> Result<Option<Refined>, String> {
    use cmaes::{CMAESOptions, DVector};
    if !glaux_ml::clap::available() {
        return Ok(None);
    }
    let tw: Vec<_> = intent
        .toward
        .iter()
        .filter_map(|w| glaux_ml::clap::find_word(w))
        .collect();
    let aw: Vec<_> = intent
        .away
        .iter()
        .filter_map(|w| glaux_ml::clap::find_word(w))
        .collect();
    if tw.is_empty() && aw.is_empty() {
        return Ok(None);
    }
    let dims: Vec<usize> = (0..7).filter(|&i| intent.feels[i] != 0.0).collect();
    if dims.is_empty() {
        return Ok(None);
    }
    // 言葉の意図は覆さない: 向きは変えず、強さは半分までしか弱めない(採点が言葉と逆の向きを好むことがあるため)
    let offsets_of = |x: &[f64]| {
        let mut o = [0.0; 7];
        for (k, &i) in dims.iter().enumerate() {
            let want = intent.feels[i];
            let total = (want + x[k].clamp(-0.25, 0.25)).clamp(-0.5, 0.5);
            let total = if total * want <= 0.0 || total.abs() < want.abs() * 0.5 {
                want * 0.5
            } else {
                total
            };
            o[i] = total - want;
        }
        o
    };
    let score = |x: &[f64]| -> Result<f64, String> {
        let (d, fx) = patch(intent, &offsets_of(x));
        let mut y = render_patch(intent, &d, &fx)?;
        // 音量をそろえる(大きさだけで印象を動かさない)
        let r = (y.iter().map(|v| v * v).sum::<f32>() / y.len().max(1) as f32).sqrt();
        if r < 1e-6 {
            return Ok(-10.0);
        }
        y.iter_mut().for_each(|v| *v *= 0.1 / r);
        let e = glaux_ml::clap::embed(&y, 48_000.0).map_err(|e| e.to_string())?;
        let mean = |ws: &[&glaux_ml::clap::VocabWord]| {
            if ws.is_empty() {
                0.0
            } else {
                ws.iter().map(|w| w.z(&e) as f64).sum::<f64>() / ws.len() as f64
            }
        };
        Ok(mean(&tw) - mean(&aw))
    };
    let x0 = vec![0.0; dims.len()];
    let before = score(&x0)?;
    let objective = |x: &DVector<f64>| -> f64 {
        let s = score(x.as_slice()).unwrap_or(-10.0);
        let out: f64 = x
            .iter()
            .map(|v| (v.abs() - 0.25).max(0.0).powi(2))
            .sum::<f64>()
            * 50.0;
        -s + out
    };
    let mut cma = CMAESOptions::new(x0.clone(), 0.1)
        .population_size(4)
        .max_function_evals(max_evals.clamp(4, 40))
        .seed(7)
        .build(objective)
        .map_err(|e| format!("{e:?}"))?;
    let result = cma.run();
    let evaluations = cma.function_evals() + 1;
    let best = result
        .overall_best
        .map(|b| b.point.as_slice().to_vec())
        .unwrap_or_else(|| x0.clone());
    let after = score(&best)?;
    let (offsets, after) = if after > before {
        (offsets_of(&best), after)
    } else {
        ([0.0; 7], before)
    };
    let r2 = |v: f64| (v * 100.0).round() / 100.0;
    Ok(Some(Refined {
        offsets: offsets.map(r2),
        score_before: r2(before),
        score_after: r2(after),
        evaluations,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_roles_feels_modifiers_and_negation() {
        let i = read("暗くて太いベース、少し揺れる");
        assert_eq!(i.role, Role::Bass);
        assert!(i.feels[feel_index("brightness")] < 0.0);
        assert!(i.feels[feel_index("body")] > 0.0);
        // 「少し」揺れる = 弱め
        let m = i.feels[feel_index("motion")];
        assert!(m > 0.0 && m < 0.2, "{m}");
        assert!(i.toward.contains(&"dark") && i.toward.contains(&"thick and fat"));
        // 否定: リバーブなし → 空間を下げて外す
        let i = read("リバーブなしの鋭いリード");
        assert_eq!(i.role, Role::Lead);
        assert!(i.feels[feel_index("space")] < 0.0);
        assert!(i.extras.contains(&Extra::NoSpace));
        assert!(i.away.contains(&"reverberant"));
        // 英語と強め
        let i = read("a very bright wide pad");
        assert_eq!(i.role, Role::Pad);
        assert!(i.feels[feel_index("brightness")] > 0.3);
        assert!(i.feels[feel_index("width")] > 0.0);
        // 何も分からない文はリードのまま
        assert_eq!(read("いい感じ").role, Role::Lead);
    }

    #[test]
    fn patches_follow_the_words() {
        let cutoff = |text: &str| {
            let (d, _) = patch(&read(text), &[0.0; 7]);
            d.params.get("cutoff").and_then(ParamValue::as_f64).unwrap()
        };
        assert!(cutoff("明るいベース") > cutoff("ベース"));
        assert!(cutoff("暗いベース") < cutoff("ベース"));
        // ローファイはテープ、ゲーム機は矩形 + ビットクラッシュ、リバーブなしは空間を外す
        let (_, fx) = patch(&read("ローファイなエレピ"), &[0.0; 7]);
        assert!(fx.iter().any(|e| e.source
            == glaux_core::PluginSource::Builtin {
                name: "tape".into()
            }));
        let (d, fx) = patch(&read("8bit のリード"), &[0.0; 7]);
        assert_eq!(
            d.params.get("waveform"),
            Some(&ParamValue::Enum("square".into()))
        );
        assert!(fx.iter().any(|e| e.source
            == glaux_core::PluginSource::Builtin {
                name: "bitcrush".into()
            }));
        let (_, fx) = patch(&read("リバーブなしのパッド"), &[0.0; 7]);
        assert!(fx.is_empty(), "{fx:?}");
        // 説明にエフェクトの変化も出る
        let i = read("遠いリード");
        let (d, fx) = patch(&i, &[0.0; 7]);
        assert!(
            describe_changes(&i, &d, &fx)
                .iter()
                .any(|c| c.starts_with("delay.mix") || c.contains("reverb")),
            "{:?}",
            describe_changes(&i, &d, &fx)
        );
        // 遠いリードには空間が足される(delay か reverb の mix が上がる)
        let (_, near) = patch(&read("リード"), &[0.0; 7]);
        let (_, far) = patch(&read("遠いリード"), &[0.0; 7]);
        let mix = |fx: &[Effect]| {
            fx.iter()
                .filter_map(|e| e.params.get("mix").and_then(ParamValue::as_f64))
                .sum::<f64>()
        };
        assert!(mix(&far) > mix(&near));
    }

    #[test]
    fn every_role_has_a_valid_base_and_phrase() {
        for r in [
            Role::Bass,
            Role::Lead,
            Role::Pad,
            Role::Pluck,
            Role::Keys,
            Role::Bell,
            Role::Organ,
            Role::Strings,
            Role::Brass,
            Role::Supersaw,
            Role::Wobble,
            Role::Vocal,
        ] {
            let (d, fx) = base(r);
            let glaux_core::PluginSource::Builtin { name } = &d.source else {
                panic!()
            };
            let specs = glaux_dsp::instrument_params(name).unwrap();
            for k in d.params.keys() {
                assert!(specs.iter().any(|s| s.name == k.as_str()), "{r:?}: {k}");
            }
            for e in &fx {
                let glaux_core::PluginSource::Builtin { name } = &e.source else {
                    panic!()
                };
                let specs = glaux_dsp::effect_params_spec(name).unwrap();
                for k in e.params.keys() {
                    assert!(
                        specs.iter().any(|s| s.name == k.as_str()),
                        "{r:?}: {name}.{k}"
                    );
                }
            }
            assert!(!phrase(r).is_empty());
        }
    }

    #[test]
    fn lexicon_vocab_words_exist_in_the_clap_vocabulary() {
        for l in LEXICON {
            for v in l.vocab {
                assert!(glaux_ml::clap::find_word(v).is_some(), "{v}");
            }
        }
    }
}
