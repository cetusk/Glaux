//! パラメータ。
//!
//! ファイルには **値だけ** を書く([`ParamMap`])。単位・範囲・説明といった意味情報は
//! [`ParamSpec`] としてデバイス定義側(Rust コード)が持ち、MCP の `list_params` で AI に返す。

use crate::id::{FxId, IdError};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;

/// パラメータ名 → 値。BTreeMap なので JSON 出力の順序が安定し、差分が読みやすい。
pub type ParamMap = BTreeMap<String, ParamValue>;

#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ParamValue {
    Bool(bool),
    Int(i64),
    Float(f64),
    Enum(String),
}

impl ParamValue {
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            ParamValue::Float(v) => Some(*v),
            ParamValue::Int(v) => Some(*v as f64),
            ParamValue::Bool(b) => Some(if *b { 1.0 } else { 0.0 }),
            ParamValue::Enum(_) => None,
        }
    }
}

impl From<f64> for ParamValue {
    fn from(v: f64) -> Self {
        ParamValue::Float(v)
    }
}
impl From<i64> for ParamValue {
    fn from(v: i64) -> Self {
        ParamValue::Int(v)
    }
}
impl From<bool> for ParamValue {
    fn from(v: bool) -> Self {
        ParamValue::Bool(v)
    }
}
impl From<&str> for ParamValue {
    fn from(v: &str) -> Self {
        ParamValue::Enum(v.to_owned())
    }
}

/// パラメータの所在。JSON では文字列:
/// - `device/<name>`   トラックの楽器デバイス
/// - `fx/<fx_id>/<name>` エフェクト
/// - `track/<name>`    トラック自身(`volume_db`, `pan`)
#[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Debug, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub enum ParamPath {
    Device { name: String },
    Effect { id: FxId, name: String },
    Track { name: String },
}

impl ParamPath {
    pub fn device(name: impl Into<String>) -> Self {
        ParamPath::Device { name: name.into() }
    }
    pub fn effect(id: FxId, name: impl Into<String>) -> Self {
        ParamPath::Effect {
            id,
            name: name.into(),
        }
    }
    pub fn track(name: impl Into<String>) -> Self {
        ParamPath::Track { name: name.into() }
    }

    pub fn parse(s: &str) -> Result<Self, IdError> {
        let err = || IdError {
            id: s.to_owned(),
            expected: "device/<name> | fx/<fx_id>/<name> | track/<name>",
        };
        let mut parts = s.splitn(3, '/');
        match (parts.next(), parts.next(), parts.next()) {
            (Some("device"), Some(name), None) if !name.is_empty() => Ok(Self::device(name)),
            (Some("track"), Some(name), None) if !name.is_empty() => Ok(Self::track(name)),
            (Some("fx"), Some(id), Some(name)) if !name.is_empty() => {
                Ok(Self::effect(FxId::parse(id)?, name))
            }
            _ => Err(err()),
        }
    }
}

impl fmt::Display for ParamPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ParamPath::Device { name } => write!(f, "device/{name}"),
            ParamPath::Effect { id, name } => write!(f, "fx/{id}/{name}"),
            ParamPath::Track { name } => write!(f, "track/{name}"),
        }
    }
}

impl TryFrom<String> for ParamPath {
    type Error = IdError;
    fn try_from(s: String) -> Result<Self, IdError> {
        Self::parse(&s)
    }
}

impl From<ParamPath> for String {
    fn from(p: ParamPath) -> String {
        p.to_string()
    }
}

/// パラメータの意味情報(ファイルには書かない。デバイス定義が持つ)。
#[derive(Clone, Debug, Serialize)]
pub struct ParamSpec {
    pub name: &'static str,
    pub display_name: &'static str,
    pub unit: Option<&'static str>,
    pub range: ParamRange,
    /// AI 向けの説明。「下げると音がこもり、上げると明るくなる」のように
    /// 聴感上の効果を書くと AI の操作精度が上がる。
    pub description: &'static str,
}

#[derive(Clone, Debug, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ParamRange {
    Float {
        min: f64,
        max: f64,
        default: f64,
        #[serde(skip_serializing_if = "Option::is_none")]
        skew: Option<f64>,
    },
    Int {
        min: i64,
        max: i64,
        default: i64,
    },
    Bool {
        default: bool,
    },
    Enum {
        choices: &'static [&'static str],
        default: &'static str,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_roundtrip() {
        for s in [
            "device/filter.cutoff",
            "fx/fx_a1b2c3/threshold_db",
            "track/volume_db",
        ] {
            let p = ParamPath::parse(s).unwrap();
            assert_eq!(p.to_string(), s);
        }
        assert!(ParamPath::parse("nope/x").is_err());
        assert!(ParamPath::parse("fx/bad/x").is_err());
    }

    #[test]
    fn value_untagged() {
        let v: ParamValue = serde_json::from_str("800").unwrap();
        assert_eq!(v, ParamValue::Int(800));
        let v: ParamValue = serde_json::from_str("800.0").unwrap();
        assert_eq!(v, ParamValue::Float(800.0));
        let v: ParamValue = serde_json::from_str("\"saw\"").unwrap();
        assert_eq!(v, ParamValue::Enum("saw".into()));
    }
}
