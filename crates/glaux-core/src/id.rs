//! 型付き ID。
//!
//! JSON 上は `"trk_a1b2c3"` のような文字列だが、Rust 上は型が分かれているので
//! `ClipId` を `TrackId` の引数に渡すといったミスをコンパイラが弾く。
//! 先頭の種別プレフィックスは AI が読んだときにも種類が分かる。

use rand::Rng;
use serde::{Deserialize, Serialize};
use std::fmt;

pub(crate) fn random_base36(len: usize) -> String {
    const CHARS: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    let mut rng = rand::thread_rng();
    (0..len)
        .map(|_| CHARS[rng.gen_range(0..CHARS.len())] as char)
        .collect()
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("invalid id `{id}`: expected `{expected}`")]
pub struct IdError {
    pub id: String,
    pub expected: &'static str,
}

macro_rules! define_id {
    ($(#[$doc:meta])* $name:ident, $prefix:literal) => {
        $(#[$doc])*
        #[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Debug, Serialize, Deserialize)]
        #[serde(try_from = "String", into = "String")]
        pub struct $name(String);

        impl $name {
            pub const PREFIX: &'static str = $prefix;

            /// ランダムな新規 ID を生成する。
            pub fn new() -> Self {
                Self(format!("{}_{}", $prefix, random_base36(6)))
            }

            /// 文字列から検証付きで生成する。
            pub fn parse(s: &str) -> Result<Self, IdError> {
                let rest = s.strip_prefix($prefix).and_then(|r| r.strip_prefix('_'));
                match rest {
                    Some(r) if !r.is_empty() && r.bytes().all(|b| b.is_ascii_alphanumeric()) => {
                        Ok(Self(s.to_owned()))
                    }
                    _ => Err(IdError { id: s.to_owned(), expected: concat!($prefix, "_<alnum>") }),
                }
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }

        impl TryFrom<String> for $name {
            type Error = IdError;
            fn try_from(s: String) -> Result<Self, IdError> {
                Self::parse(&s)
            }
        }

        impl std::str::FromStr for $name {
            type Err = IdError;
            fn from_str(s: &str) -> Result<Self, IdError> {
                Self::parse(s)
            }
        }

        impl From<$name> for String {
            fn from(id: $name) -> String {
                id.0
            }
        }
    };
}

define_id!(/// トラック
    TrackId, "trk");
define_id!(/// クリップ
    ClipId, "clp");
define_id!(/// ノート
    NoteId, "nt");
define_id!(/// エフェクト
    FxId, "fx");
define_id!(/// 履歴エントリ
    EntryId, "hst");

/// 音声アセット。内容ハッシュで参照する: `sha256:<hex>`。
#[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Debug, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct AssetId(String);

impl AssetId {
    pub const PREFIX: &'static str = "sha256";

    pub fn from_sha256_hex(hex: &str) -> Result<Self, IdError> {
        Self::parse(&format!("sha256:{hex}"))
    }

    pub fn parse(s: &str) -> Result<Self, IdError> {
        match s.strip_prefix("sha256:") {
            Some(h) if !h.is_empty() && h.bytes().all(|b| b.is_ascii_hexdigit()) => {
                Ok(Self(s.to_ascii_lowercase()))
            }
            _ => Err(IdError {
                id: s.to_owned(),
                expected: "sha256:<hex>",
            }),
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for AssetId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl TryFrom<String> for AssetId {
    type Error = IdError;
    fn try_from(s: String) -> Result<Self, IdError> {
        Self::parse(&s)
    }
}

impl From<AssetId> for String {
    fn from(id: AssetId) -> String {
        id.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_and_reject() {
        assert!(TrackId::parse("trk_abc123").is_ok());
        assert!(TrackId::parse("clp_abc123").is_err());
        assert!(TrackId::parse("trk_").is_err());
        assert!(AssetId::parse("sha256:ab12").is_ok());
        assert!(AssetId::parse("sha256:zz").is_err());
    }

    #[test]
    fn serde_roundtrip_as_plain_string() {
        let id = ClipId::new();
        let json = serde_json::to_string(&id).unwrap();
        assert_eq!(json, format!("\"{}\"", id));
        let back: ClipId = serde_json::from_str(&json).unwrap();
        assert_eq!(id, back);
        assert!(serde_json::from_str::<ClipId>("\"trk_x1\"").is_err());
    }
}
