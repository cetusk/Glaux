//! 音声アセット。プロジェクト構造からは内容ハッシュで参照する。

use serde::{Deserialize, Serialize};

#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct Asset {
    /// プロジェクトフォルダからの相対パス(例: `audio/kick.wav`)
    pub path: String,
    pub sample_rate: u32,
    pub channels: u16,
    /// サンプルフレーム数(チャンネルあたり)
    pub frames: u64,
}
