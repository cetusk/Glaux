//! 楽器の `ParamSpec` 定義と、`ParamMap`(値のみ)からの焼き込み。
//!
//! description は AI が読む唯一の「つまみの説明書」。聴感上の効果を書くこと。

use crate::drum::DrumParams;
use crate::pluck::PluckParams;
use crate::subtractive::{SubtractiveParams, Waveform};
use crate::voice::{InstrumentKind, InstrumentParams};
use glaux_core::{Device, ParamMap, ParamRange, ParamSpec, ParamValue, PluginSource};

pub static SUBTRACTIVE_SPECS: &[ParamSpec] = &[
    ParamSpec {
        name: "waveform",
        display_name: "波形",
        unit: None,
        range: ParamRange::Enum {
            choices: &["saw", "square", "triangle", "sine"],
            default: "saw",
        },
        description: "音の基本キャラクター。saw は明るく厚い、square は木管っぽく中空、\
            triangle は丸く柔らかい、sine は最も澄んだ純音。",
    },
    ParamSpec {
        name: "cutoff",
        display_name: "カットオフ",
        unit: Some("Hz"),
        range: ParamRange::Float {
            min: 40.0,
            max: 12000.0,
            default: 8000.0,
            skew: Some(0.3),
        },
        description: "下げると音がこもって暗くなり、上げると明るく開ける。\
            ベースは 200〜800、パッドは 1000〜3000、リードは 3000 以上が目安。",
    },
    ParamSpec {
        name: "resonance",
        display_name: "レゾナンス",
        unit: None,
        range: ParamRange::Float {
            min: 0.0,
            max: 0.95,
            default: 0.15,
            skew: None,
        },
        description: "カットオフ付近を強調する癖の強さ。上げるとミョンミョンした\
            シンセらしい鳴りになり、上げすぎるとピーキーで耳に刺さる。",
    },
    ParamSpec {
        name: "attack",
        display_name: "アタック",
        unit: Some("s"),
        range: ParamRange::Float {
            min: 0.001,
            max: 2.0,
            default: 0.005,
            skew: Some(0.3),
        },
        description: "音の立ち上がりの速さ。短いとパーカッシブ、長いとふわっと\
            立ち上がるパッド向きになる。",
    },
    ParamSpec {
        name: "decay",
        display_name: "ディケイ",
        unit: Some("s"),
        range: ParamRange::Float {
            min: 0.01,
            max: 3.0,
            default: 0.15,
            skew: Some(0.3),
        },
        description: "立ち上がり後にサスティンレベルまで落ちる時間。\
            短いとプラック(はじいた)感が出る。",
    },
    ParamSpec {
        name: "sustain",
        display_name: "サスティン",
        unit: None,
        range: ParamRange::Float {
            min: 0.0,
            max: 1.0,
            default: 0.7,
            skew: None,
        },
        description: "押している間の音量。0 に近いとプラック/スタッカート的、\
            1 に近いとオルガンのように持続する。",
    },
    ParamSpec {
        name: "release",
        display_name: "リリース",
        unit: Some("s"),
        range: ParamRange::Float {
            min: 0.01,
            max: 4.0,
            default: 0.2,
            skew: Some(0.3),
        },
        description: "ノートを離した後の余韻の長さ。長いと残響感が出るが、\
            速いフレーズでは音が濁る。",
    },
    ParamSpec {
        name: "filter_env",
        display_name: "フィルターエンベロープ",
        unit: None,
        range: ParamRange::Float {
            min: 0.0,
            max: 1.0,
            default: 0.35,
            skew: None,
        },
        description: "音の出だしでカットオフが一時的に開く量。上げると\
            「ビャッ」というシンセらしいアタック感が付く。",
    },
    ParamSpec {
        name: "unison",
        display_name: "ユニゾン",
        unit: None,
        range: ParamRange::Int {
            min: 1,
            max: 7,
            default: 1,
        },
        description: "同じ音を微妙にピッチをずらして重ねる本数。3〜7 + detune で\
            EDM の分厚い supersaw になる。1 で従来どおり。",
    },
    ParamSpec {
        name: "detune",
        display_name: "デチューン",
        unit: Some("cents"),
        range: ParamRange::Float {
            min: 0.0,
            max: 60.0,
            default: 12.0,
            skew: None,
        },
        description: "ユニゾンの広がり(半音=100)。上げるほど太くうねるが、\
            上げすぎると音程感が薄れる。supersaw は 15〜30 が目安。",
    },
    ParamSpec {
        name: "sub",
        display_name: "サブオシレータ",
        unit: None,
        range: ParamRange::Float {
            min: 0.0,
            max: 1.0,
            default: 0.0,
            skew: None,
        },
        description: "1 オクターブ下のサイン波を混ぜる量。ベースの土台・胸に来る低域。\
            EDM ベースは 0.5〜1.0 が定番。",
    },
    ParamSpec {
        name: "noise",
        display_name: "ノイズ",
        unit: None,
        range: ParamRange::Float {
            min: 0.0,
            max: 1.0,
            default: 0.0,
            skew: None,
        },
        description: "ホワイトノイズを混ぜる量。息っぽさ・ざらつき・シュワッとした質感。",
    },
    ParamSpec {
        name: "gain_db",
        display_name: "ゲイン",
        unit: Some("dB"),
        range: ParamRange::Float {
            min: -24.0,
            max: 6.0,
            default: -9.0,
            skew: None,
        },
        description: "楽器自体の音量。トラック音量と別。和音を弾くと音が重なるので\
            クリップするなら下げる。",
    },
];

pub static DRUM_SPECS: &[ParamSpec] = &[
    ParamSpec {
        name: "gain_db",
        display_name: "ゲイン",
        unit: Some("dB"),
        range: ParamRange::Float {
            min: -24.0,
            max: 6.0,
            default: -6.0,
            skew: None,
        },
        description: "ドラムキット全体の音量。",
    },
    ParamSpec {
        name: "decay",
        display_name: "ディケイ",
        unit: None,
        range: ParamRange::Float {
            min: 0.25,
            max: 4.0,
            default: 1.0,
            skew: Some(0.5),
        },
        description: "全パーツの減衰時間の倍率。下げるとタイトで締まった音、\
            上げるとルーズで残響っぽくなる。",
    },
    ParamSpec {
        name: "tone",
        display_name: "トーン",
        unit: None,
        range: ParamRange::Float {
            min: 0.0,
            max: 1.0,
            default: 0.5,
            skew: None,
        },
        description: "明るさ。上げるとキックのクリックやスネア・ハットの\
            高域成分が増えて抜けが良くなり、下げると丸くローファイになる。",
    },
    ParamSpec {
        name: "tune",
        display_name: "チューニング",
        unit: Some("semitones"),
        range: ParamRange::Float {
            min: -12.0,
            max: 12.0,
            default: 0.0,
            skew: None,
        },
        description: "キック・タムなど音程を持つパーツのピッチを半音単位でずらす。",
    },
];

pub static PLUCK_SPECS: &[ParamSpec] = &[
    ParamSpec {
        name: "decay",
        display_name: "ディケイ",
        unit: Some("s"),
        range: ParamRange::Float {
            min: 0.05,
            max: 8.0,
            default: 2.5,
            skew: Some(0.4),
        },
        description: "弦の鳴りの長さ。短いとミュートっぽく歯切れよく、\
            長いとサスティンが伸びてアルペジオが響き合う。",
    },
    ParamSpec {
        name: "brightness",
        display_name: "明るさ",
        unit: None,
        range: ParamRange::Float {
            min: 0.0,
            max: 0.95,
            default: 0.5,
            skew: None,
        },
        description: "弦の明るさ(高域がどれだけ長く残るか)。上げるとスチール弦の\
            ジャキッとした鳴り、下げるとナイロン弦のような丸い音。",
    },
    ParamSpec {
        name: "pick",
        display_name: "ピック",
        unit: None,
        range: ParamRange::Float {
            min: 0.0,
            max: 1.0,
            default: 0.6,
            skew: None,
        },
        description: "ピッキングの硬さ。上げるとアタックが硬くアグレッシブに、\
            下げると指弾きのように柔らかくなる。",
    },
    ParamSpec {
        name: "gain_db",
        display_name: "ゲイン",
        unit: Some("dB"),
        range: ParamRange::Float {
            min: -24.0,
            max: 6.0,
            default: -6.0,
            skew: None,
        },
        description: "楽器自体の音量。トラック音量と別。",
    },
];

pub static SAMPLER_SPECS: &[ParamSpec] = &[
    ParamSpec {
        name: "root",
        display_name: "ルート音程",
        unit: None,
        range: ParamRange::Int {
            min: 0,
            max: 127,
            default: 60,
        },
        description: "サンプル自身の音程(MIDI ノート番号。60 = C4)。この音で等速再生になり、\
            離れるほどピッチ変換で速く/遅く再生される。サンプルの実音に合わせること。",
    },
    ParamSpec {
        name: "release_ms",
        display_name: "リリース",
        unit: Some("ms"),
        range: ParamRange::Float {
            min: 5.0,
            max: 2000.0,
            default: 80.0,
            skew: Some(0.4),
        },
        description: "ノート終了後のフェード時間。短いとブツッと切れず自然に止まり、\
            長いと余韻が重なる。",
    },
    ParamSpec {
        name: "gain_db",
        display_name: "ゲイン",
        unit: Some("dB"),
        range: ParamRange::Float {
            min: -24.0,
            max: 12.0,
            default: 0.0,
            skew: None,
        },
        description: "楽器自体の音量。トラック音量と別。",
    },
];

pub static SF2_SPECS: &[ParamSpec] = &[ParamSpec {
    name: "gain_db",
    display_name: "ゲイン",
    unit: Some("dB"),
    range: ParamRange::Float {
        min: -24.0,
        max: 12.0,
        default: 0.0,
        skew: None,
    },
    description: "楽器自体の音量。トラック音量と別。",
}];

/// 奏法(アーティキュレーション)の楽器別説明。
/// 同じ奏法でも楽器によって効き方が違う(または効かない)ので、楽器ごとに持つ。
#[derive(Clone, Debug, serde::Serialize)]
pub struct ArticulationInfo {
    /// JSON に書く値(Note.articulation)
    pub name: &'static str,
    /// ピアノロールのショートカットキー
    pub key: &'static str,
    pub display_name: &'static str,
    /// この楽器での聴感上の効果
    pub description: &'static str,
}

const ART_STACCATO: ArticulationInfo = ArticulationInfo {
    name: "staccato",
    key: "S",
    display_name: "スタッカート",
    description: "音価の半分で切る歯切れのよい発音。",
};
const ART_ACCENT: ArticulationInfo = ArticulationInfo {
    name: "accent",
    key: "A",
    display_name: "アクセント",
    description: "その音だけ強く目立たせる。",
};
const ART_VIBRATO: ArticulationInfo = ArticulationInfo {
    name: "vibrato",
    key: "V",
    display_name: "ビブラート",
    description: "音の後半にかけて深くなるピッチの揺れ。ロングトーンの表情付け。",
};
const ART_BEND: ArticulationInfo = ArticulationInfo {
    name: "bend",
    key: "B",
    display_name: "チョーキング",
    description: "全音下から書かれた音程へ滑り上がる。フレーズの決め音に。",
};

pub static SUBTRACTIVE_ARTS: &[ArticulationInfo] = &[
    ArticulationInfo {
        name: "palm_mute",
        key: "M",
        display_name: "ミュート",
        description: "カットオフを絞って速く減衰させた、こもった短い音。シンセの刻みに。",
    },
    ART_STACCATO,
    ART_ACCENT,
    ART_VIBRATO,
    ART_BEND,
];
pub static DRUM_ARTS: &[ArticulationInfo] = &[ArticulationInfo {
    name: "accent",
    key: "A",
    display_name: "アクセント",
    description: "そのヒットだけ強く。ゴーストノートとの対比でグルーヴを作る。",
}];
pub static PLUCK_ARTS: &[ArticulationInfo] = &[
    ArticulationInfo {
        name: "palm_mute",
        key: "M",
        display_name: "ブリッジミュート",
        description: "掌で弦を押さえた「ズクズク」した刻み。メタルのリフの主役(+amp)。",
    },
    ART_STACCATO,
    ART_ACCENT,
    ART_VIBRATO,
    ART_BEND,
];
pub static SAMPLER_ARTS: &[ArticulationInfo] = &[ART_STACCATO, ART_ACCENT, ART_VIBRATO, ART_BEND];
pub static SF2_ARTS: &[ArticulationInfo] = &[ART_STACCATO, ART_ACCENT, ART_VIBRATO, ART_BEND];

/// 楽器名 → 対応する奏法の一覧。載っていない奏法を付けてもエラーにはならないが
/// 音への効果はない(no-op)。
pub fn articulations_for(instrument: &str) -> &'static [ArticulationInfo] {
    match instrument {
        "drum" => DRUM_ARTS,
        "pluck" => PLUCK_ARTS,
        "sampler" => SAMPLER_ARTS,
        "sf2" => SF2_ARTS,
        _ => SUBTRACTIVE_ARTS,
    }
}

/// 楽器カタログの 1 行(MCP の `list_params` がそのまま返す)。
#[derive(Clone, Debug, serde::Serialize)]
pub struct InstrumentInfo {
    pub name: &'static str,
    pub description: &'static str,
    pub params: &'static [ParamSpec],
    /// この楽器で効く奏法(Note.articulation)
    pub articulations: &'static [ArticulationInfo],
}

/// 内蔵楽器の一覧。
pub fn instrument_catalog() -> Vec<InstrumentInfo> {
    vec![
        InstrumentInfo {
            name: "subtractive",
            description: "減算方式シンセ。ベース・リード・パッド・プラックなど\
                メロディ楽器全般に使う。device 未設定トラックの既定音源。",
            params: SUBTRACTIVE_SPECS,
            articulations: SUBTRACTIVE_ARTS,
        },
        InstrumentInfo {
            name: "drum",
            description: "ドラムシンセ。MIDI ノート番号(GM 配置)で音色が決まる: \
                36=キック, 38=スネア, 39=クラップ, 42=クローズドハット, 46=オープンハット, \
                41〜50=タム, 49/51=シンバル, 55=リバースクラッシュ(盛り上がり前の\
                ビルドアップに。ノートの開始位置から立ち上がり、鳴り終わりをドロップ頭に合わせる)。\
                ドラムトラックには set_device でこれを設定する。",
            params: DRUM_SPECS,
            articulations: DRUM_ARTS,
        },
        InstrumentInfo {
            name: "sf2",
            description: "SoundFont(.sf2)のプリセットを鳴らすマルチサンプラー。\
                音域・ベロシティごとの多段サンプル + ループで、ピアノ・ストリングス・\
                ギター・ブラスなど「本物っぽい楽器一式」が使える。\
                list_soundfonts で置いてある .sf2 とプリセットを確認し、\
                set_soundfont_instrument でトラックに設定する。",
            params: SF2_SPECS,
            articulations: SF2_ARTS,
        },
        InstrumentInfo {
            name: "sampler",
            description: "単一サンプル再生(ワンショット)。WAV を root 基準のピッチ変換で\
                鳴らす。実録の質感(本物のギター、ボーカルチョップ、生ドラムの\
                ワンショット等)はこれを使う。導入は import_sample ツール\
                (UI では音作りビューの「サンプルを読み込み」)。",
            params: SAMPLER_SPECS,
            articulations: SAMPLER_ARTS,
        },
        InstrumentInfo {
            name: "pluck",
            description: "撥弦の物理モデル(Karplus-Strong)。アコースティックギター・\
                ベース・ハープなど「弾く弦」の音はこれを使う。エレキギターは pluck + \
                distortion(メタルの刻みはさらにノートに articulation: palm_mute)。\
                シンセ的なプラックではなく本物の弦の減衰が欲しいときの第一候補。",
            params: PLUCK_SPECS,
            articulations: PLUCK_ARTS,
        },
    ]
}

/// 楽器名 → ParamSpec 一覧。
pub fn instrument_params(name: &str) -> Option<&'static [ParamSpec]> {
    match name {
        "subtractive" => Some(SUBTRACTIVE_SPECS),
        "drum" => Some(DRUM_SPECS),
        "pluck" => Some(PLUCK_SPECS),
        "sampler" => Some(SAMPLER_SPECS),
        "sf2" => Some(SF2_SPECS),
        _ => None,
    }
}

fn get_f32(map: &ParamMap, specs: &[ParamSpec], name: &str) -> f32 {
    if let Some(v) = map.get(name).and_then(ParamValue::as_f64) {
        return v as f32;
    }
    match specs.iter().find(|s| s.name == name).map(|s| &s.range) {
        Some(ParamRange::Float { default, .. }) => *default as f32,
        Some(ParamRange::Int { default, .. }) => *default as f32,
        _ => 0.0,
    }
}

fn get_enum<'a>(map: &'a ParamMap, specs: &[ParamSpec], name: &'a str) -> &'a str {
    if let Some(ParamValue::Enum(s)) = map.get(name) {
        return s;
    }
    match specs.iter().find(|s| s.name == name).map(|s| &s.range) {
        Some(ParamRange::Enum { default, .. }) => default,
        _ => "",
    }
}

fn db_to_amp(db: f32) -> f32 {
    10.0_f32.powf(db / 20.0)
}

static EMPTY_PARAMS: ParamMap = ParamMap::new();

/// SoundFont マルチサンプラーの焼き込み(ゾーンはエンジン側で構築して渡す)。
pub fn bake_sf2(
    map: &ParamMap,
    zones: std::sync::Arc<Vec<crate::multi::Zone>>,
) -> crate::multi::MultiSamplerParams {
    let s = SF2_SPECS;
    crate::multi::MultiSamplerParams {
        zones,
        gain: db_to_amp(get_f32(map, s, "gain_db").clamp(-24.0, 12.0)),
    }
}

/// サンプラーの焼き込み(波形はエンジン側で読み込んで渡す)。
pub fn bake_sampler(
    map: &ParamMap,
    data: std::sync::Arc<crate::sampler::SampleData>,
    sample_rate: f32,
) -> crate::sampler::SamplerParams {
    let s = SAMPLER_SPECS;
    let release_ms = get_f32(map, s, "release_ms").clamp(5.0, 2000.0);
    crate::sampler::SamplerParams {
        data,
        root: get_f32(map, s, "root").clamp(0.0, 127.0) as u8,
        gain: db_to_amp(get_f32(map, s, "gain_db").clamp(-24.0, 12.0)),
        release_coef: 1.0 / (release_ms * 0.001 * sample_rate),
    }
}

/// トラックの `Device` から再生用パラメータを焼き込む。
/// device が無い場合は既定の subtractive。内蔵以外(CLAP / サンプラー)や
/// 未知の名前も当面 subtractive で代用する。
pub fn bake_instrument(device: Option<&Device>) -> (InstrumentKind, InstrumentParams) {
    let (name, map): (&str, &ParamMap) = match device {
        Some(d) => match &d.source {
            PluginSource::Builtin { name } => (name.as_str(), &d.params),
            _ => (crate::DEFAULT_INSTRUMENT, &d.params),
        },
        None => (crate::DEFAULT_INSTRUMENT, &EMPTY_PARAMS),
    };

    match name {
        "pluck" => {
            let s = PLUCK_SPECS;
            let p = PluckParams {
                decay: get_f32(map, s, "decay").clamp(0.05, 8.0),
                brightness: get_f32(map, s, "brightness").clamp(0.0, 0.95),
                pick: get_f32(map, s, "pick").clamp(0.0, 1.0),
                gain: db_to_amp(get_f32(map, s, "gain_db").clamp(-24.0, 6.0)),
            };
            (InstrumentKind::Pluck, InstrumentParams::Pluck(p))
        }
        "drum" => {
            let s = DRUM_SPECS;
            let p = DrumParams {
                gain: db_to_amp(get_f32(map, s, "gain_db")),
                decay: get_f32(map, s, "decay").clamp(0.25, 4.0),
                tone: get_f32(map, s, "tone").clamp(0.0, 1.0),
                tune: get_f32(map, s, "tune").clamp(-12.0, 12.0),
            };
            (InstrumentKind::Drum, InstrumentParams::Drum(p))
        }
        _ => {
            let s = SUBTRACTIVE_SPECS;
            let p = SubtractiveParams {
                waveform: Waveform::parse(get_enum(map, s, "waveform")),
                cutoff: get_f32(map, s, "cutoff").clamp(40.0, 12000.0),
                resonance: get_f32(map, s, "resonance").clamp(0.0, 0.95),
                attack: get_f32(map, s, "attack").clamp(0.001, 2.0),
                decay: get_f32(map, s, "decay").clamp(0.01, 3.0),
                sustain: get_f32(map, s, "sustain").clamp(0.0, 1.0),
                release: get_f32(map, s, "release").clamp(0.01, 4.0),
                filter_env: get_f32(map, s, "filter_env").clamp(0.0, 1.0),
                unison: get_f32(map, s, "unison").clamp(1.0, 7.0) as u8,
                detune_cents: get_f32(map, s, "detune").clamp(0.0, 60.0),
                sub: get_f32(map, s, "sub").clamp(0.0, 1.0),
                noise: get_f32(map, s, "noise").clamp(0.0, 1.0),
                gain: db_to_amp(get_f32(map, s, "gain_db").clamp(-24.0, 6.0)),
            };
            (
                InstrumentKind::Subtractive,
                InstrumentParams::Subtractive(p),
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bake_defaults_when_no_device() {
        let (kind, params) = bake_instrument(None);
        assert_eq!(kind, InstrumentKind::Subtractive);
        let InstrumentParams::Subtractive(p) = params else {
            panic!("expected subtractive");
        };
        assert_eq!(p.waveform, Waveform::Saw);
        assert!((p.cutoff - 8000.0).abs() < 1e-3);
    }

    #[test]
    fn bake_overrides_and_clamps() {
        let mut device = Device::builtin("subtractive");
        device.params.insert("cutoff".into(), 500.0.into());
        device.params.insert("waveform".into(), "square".into());
        device.params.insert("resonance".into(), 99.0.into()); // clamp される
        let (_, params) = bake_instrument(Some(&device));
        let InstrumentParams::Subtractive(p) = params else {
            panic!()
        };
        assert_eq!(p.waveform, Waveform::Square);
        assert!((p.cutoff - 500.0).abs() < 1e-3);
        assert!(p.resonance <= 0.95);
    }

    #[test]
    fn bake_drum_and_unknown_falls_back() {
        let (kind, _) = bake_instrument(Some(&Device::builtin("drum")));
        assert_eq!(kind, InstrumentKind::Drum);
        let (kind, _) = bake_instrument(Some(&Device::builtin("no_such_synth")));
        assert_eq!(kind, InstrumentKind::Subtractive);
    }

    #[test]
    fn catalog_names_resolve() {
        for info in instrument_catalog() {
            assert!(instrument_params(info.name).is_some());
            assert!(!info.params.is_empty());
        }
    }
}
