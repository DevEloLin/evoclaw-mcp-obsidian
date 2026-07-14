//! Error types for the Obsidian knowledge-base MCP server.

use std::path::PathBuf;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("vault path does not exist or is not accessible: {0}")]
    VaultNotFound(PathBuf),

    #[error("vault path is not a directory: {0}")]
    NotADirectory(PathBuf),

    /// A user-supplied note path resolved outside the vault root.
    #[error("path escapes the vault root: {0}")]
    PathEscape(String),

    /// A requested note path was not found under the vault.
    #[error("note not found in vault: {0}")]
    NoteNotFound(String),

    #[error("invalid glob pattern: {0}")]
    Glob(#[from] globset::Error),

    #[error(transparent)]
    Io(#[from] std::io::Error),

    #[error(transparent)]
    Tantivy(#[from] tantivy::TantivyError),

    #[error("index directory error: {0}")]
    OpenDir(#[from] tantivy::directory::error::OpenDirectoryError),

    #[error("manifest (de)serialization error: {0}")]
    Json(#[from] serde_json::Error),
}
