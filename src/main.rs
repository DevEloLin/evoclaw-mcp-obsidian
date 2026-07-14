//! evoclaw-mcp-obsidian — a BM25 knowledge-base MCP server over an Obsidian
//! vault. Speaks MCP over stdio; spawned on demand by the EvoClaw MCP client.

mod config;
mod error;
mod index;
mod mcp;
mod vault;

use clap::Parser;
use rmcp::{transport::stdio, ServiceExt};

use crate::config::{Cli, Config};
use crate::mcp::ObsidianService;

#[tokio::main]
async fn main() -> eyre::Result<()> {
    // Logs go to stderr; stdout is reserved for the MCP JSON-RPC stream.
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info".into()),
        )
        .init();

    let cli = Cli::parse();
    let config = Config::from_cli(cli)?;
    tracing::info!(vault = %config.vault.display(), "starting evoclaw-mcp-obsidian");

    // Build (or incrementally refresh) the index before serving.
    let index = index::KbIndex::open_or_build(&config)?;
    tracing::info!(cache = %config.cache_dir.display(), "index ready");

    let service = ObsidianService::new(config, index).serve(stdio()).await?;
    service.waiting().await?;
    Ok(())
}
