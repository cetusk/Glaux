//! glaux-mcp のライブラリ部。
//!
//! - [`store`]: `MySong.glaux/` フォルダの読み書き
//! - [`actor`]: `Session` を所有するアクタースレッドとそのハンドル
//! - [`plan_store`]: 旋律の計画 `plans.json` と、曲とは別の計画の履歴の読み書き
//! - [`server`]: MCP ツール定義(rmcp)
//!
//! バイナリ(`main.rs`)はこれらを stdio トランスポートで束ねるだけ。
//! Tauri アプリからも同じ [`actor::SessionHandle`] を使う想定。

pub mod actor;
pub mod assets;
pub mod bounce;
pub mod changes;
pub mod character;
pub mod clap_presets;
pub mod compact;
pub mod design;
pub mod export;
pub mod fx_presets;
pub mod guide;
pub mod midi;
pub mod mixcritique;
pub mod models;
pub mod musicxml;
pub mod musicxml_in;
pub mod plan_store;
pub mod preset_index;
pub mod presets;
pub mod recipes;
pub mod server;
pub mod sfz_packs;
pub mod sound;
pub mod stems;
pub mod store;
pub mod transcribe;
pub mod words;
