//! glaux-mcp: Glaux プロジェクトを AI クライアントに公開する MCP サーバー(stdio)。
//!
//! ```text
//! glaux-mcp <MySong.glaux>
//! ```
//!
//! Claude Code からは:
//! ```text
//! claude mcp add glaux -- /path/to/glaux-mcp /path/to/MySong.glaux
//! ```
//!
//! stdout は MCP プロトコル専用なので、ログはすべて stderr に出す。

use anyhow::{bail, Context, Result};
use glaux_mcp::actor::SessionHandle;
use glaux_mcp::server::GlauxServer;
use glaux_mcp::store::Store;
use rmcp::{transport::stdio, ServiceExt};

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_env("GLAUX_LOG")
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let mut args = std::env::args_os().skip(1);
    let dir = match (args.next(), args.next()) {
        (Some(dir), None) => dir,
        _ => bail!("usage: glaux-mcp <MySong.glaux>"),
    };

    let (store, session) =
        Store::open_or_create(&*dir.to_string_lossy()).context("プロジェクトを開けません")?;
    tracing::info!(
        "プロジェクトを開きました: {}(履歴 {} エントリ)",
        store.dir().display(),
        session.history().len()
    );

    let handle = SessionHandle::spawn(session, store);
    let service = GlauxServer::new(handle)
        .serve(stdio())
        .await
        .context("MCP サーバーの起動に失敗")?;
    service.waiting().await?;
    Ok(())
}
