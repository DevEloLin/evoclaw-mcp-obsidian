//! CLI surface and validated runtime configuration.

use std::path::{Path, PathBuf};

use clap::Parser;

use crate::error::{Error, Result};

/// Hard upper bound on results a single `kb_search` may return.
pub const RESULT_HARD_CAP: usize = 25;
const DEFAULT_MAX_RESULTS: usize = 8;
const DEFAULT_INCLUDE: &str = "**/*.md";

/// Command-line surface for the Obsidian knowledge-base MCP server.
#[derive(Debug, Parser)]
#[command(
    name = "evoclaw-mcp-obsidian",
    version,
    about = "BM25 knowledge-base MCP server over an Obsidian markdown vault"
)]
pub struct Cli {
    /// Path to the Obsidian vault (a directory of markdown notes).
    #[arg(long, env = "EVOCLAW_OBSIDIAN_VAULT")]
    pub vault: PathBuf,

    /// Directory for the search-index cache. Defaults to the OS cache dir,
    /// namespaced per vault.
    #[arg(long)]
    pub cache_dir: Option<PathBuf>,

    /// Glob(s) of files to include, relative to the vault. Defaults to `**/*.md`.
    #[arg(long = "include")]
    pub include: Vec<String>,

    /// Glob(s) of files to exclude, relative to the vault.
    #[arg(long = "exclude")]
    pub exclude: Vec<String>,

    /// Default maximum number of `kb_search` results.
    #[arg(long, default_value_t = DEFAULT_MAX_RESULTS)]
    pub max_results: usize,
}

/// Validated, resolved configuration. `vault` and `cache_dir` are absolute.
#[derive(Debug, Clone)]
pub struct Config {
    pub vault: PathBuf,
    pub cache_dir: PathBuf,
    pub include: Vec<String>,
    pub exclude: Vec<String>,
    pub max_results: usize,
}

impl Config {
    /// Validate a parsed CLI into a runtime config: canonicalize the vault,
    /// confirm it is a directory, resolve the cache dir, and clamp limits.
    pub fn from_cli(cli: Cli) -> Result<Self> {
        let vault = cli
            .vault
            .canonicalize()
            .map_err(|_| Error::VaultNotFound(cli.vault.clone()))?;
        if !vault.is_dir() {
            return Err(Error::NotADirectory(vault));
        }

        let cache_dir = match cli.cache_dir {
            Some(dir) => dir,
            None => default_cache_dir(&vault),
        };

        let include = if cli.include.is_empty() {
            vec![DEFAULT_INCLUDE.to_string()]
        } else {
            cli.include
        };

        let max_results = cli.max_results.clamp(1, RESULT_HARD_CAP);

        Ok(Config {
            vault,
            cache_dir,
            include,
            exclude: cli.exclude,
            max_results,
        })
    }
}

/// OS cache dir, namespaced by a stable hash of the canonical vault path so
/// distinct vaults never share an index.
fn default_cache_dir(vault: &Path) -> PathBuf {
    let base = directories::ProjectDirs::from("com", "evoclaw", "evoclaw-mcp-obsidian")
        .map(|d| d.cache_dir().to_path_buf())
        .unwrap_or_else(|| std::env::temp_dir().join("evoclaw-mcp-obsidian"));
    base.join(vault_key(vault))
}

fn vault_key(vault: &Path) -> String {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    vault.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}
