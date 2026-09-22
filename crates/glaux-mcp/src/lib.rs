//! glaux-mcp のライブラリ部。
//!
//! - [`store`]: `MySong.glaux/` フォルダの読み書き
//! - [`actor`]: `Session` を所有するアクタースレッドとそのハンドル
//! - [`server`]: MCP ツール定義(rmcp)
//!
//! バイナリ(`main.rs`)はこれらを stdio トランスポートで束ねるだけ。
//! Tauri アプリからも同じ [`actor::SessionHandle`] を使う想定。

pub mod actor;
pub mod presets;
pub mod server;
pub mod store;
