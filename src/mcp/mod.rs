//! MCP tool surface: `kb_search` and `kb_read`, backed by the BM25 index and
//! path-confined vault reads.

use std::borrow::Cow;
use std::future::Future;
use std::sync::Arc;

use rmcp::{
    handler::server::{router::tool::ToolRouter, tool::Parameters},
    model::{ErrorData as McpError, *},
    schemars, tool, tool_handler, tool_router, ServerHandler,
};
use serde::Deserialize;

use crate::config::{Config, RESULT_HARD_CAP};
use crate::index::{search, KbIndex};
use crate::vault::read;

#[derive(Clone)]
pub struct ObsidianService {
    config: Arc<Config>,
    index: Arc<KbIndex>,
    tool_router: ToolRouter<ObsidianService>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct KbSearchRequest {
    #[schemars(description = "Full-text query over the vault (BM25 ranked).")]
    pub query: String,
    #[schemars(description = "Maximum number of results to return (default from server config).")]
    pub limit: Option<usize>,
    #[schemars(description = "Only return notes tagged with ANY of these tags (Obsidian #tag or frontmatter tags).")]
    pub tags: Option<Vec<String>>,
    #[schemars(description = "Only return notes whose vault-relative path starts with this prefix.")]
    pub path_prefix: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct KbReadRequest {
    #[schemars(description = "Vault-relative path of the note to read, e.g. \"folder/note.md\".")]
    pub path: String,
    #[schemars(description = "Optional heading text; when set, return only that section of the note.")]
    pub section: Option<String>,
}

#[tool_router]
impl ObsidianService {
    pub fn new(config: Config, index: KbIndex) -> Self {
        Self {
            config: Arc::new(config),
            index: Arc::new(index),
            tool_router: Self::tool_router(),
        }
    }

    #[tool(
        description = "Search the Obsidian knowledge base with BM25 full-text ranking. Supports optional tag and path-prefix filters. Returns a JSON array of ranked hits, each with path, title, score, tags, and a snippet."
    )]
    async fn kb_search(
        &self,
        Parameters(req): Parameters<KbSearchRequest>,
    ) -> Result<CallToolResult, McpError> {
        if req.query.trim().is_empty() {
            return Err(invalid_params("query must not be empty"));
        }
        let limit = req
            .limit
            .unwrap_or(self.config.max_results)
            .clamp(1, RESULT_HARD_CAP);

        // tantivy search is synchronous CPU work; acceptable for this
        // low-concurrency stdio server.
        let hits = search::search(
            &self.index,
            &req.query,
            limit,
            req.tags.as_deref(),
            req.path_prefix.as_deref(),
        )
        .map_err(internal_error)?;

        let json = serde_json::to_string_pretty(&hits).map_err(|e| internal_error(e.into()))?;
        Ok(CallToolResult::success(vec![Content::text(json)]))
    }

    #[tool(
        description = "Read a note from the Obsidian knowledge base by its vault-relative path. Optionally return only a named section. Paths are strictly confined to the vault."
    )]
    async fn kb_read(
        &self,
        Parameters(req): Parameters<KbReadRequest>,
    ) -> Result<CallToolResult, McpError> {
        if req.path.trim().is_empty() {
            return Err(invalid_params("path must not be empty"));
        }
        let content = read::read_note(&self.config.vault, &req.path, req.section.as_deref())
            .map_err(|e| invalid_params(&e.to_string()))?;
        Ok(CallToolResult::success(vec![Content::text(content)]))
    }
}

#[tool_handler]
impl ServerHandler for ObsidianService {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            protocol_version: ProtocolVersion::V_2024_11_05,
            capabilities: ServerCapabilities::builder().enable_tools().build(),
            server_info: Implementation::from_build_env(),
            instructions: Some(
                "Obsidian knowledge base. Use kb_search to find relevant notes by query \
                 (BM25 ranking, with optional tag/path filters), then kb_read to fetch a \
                 note's full content."
                    .to_string(),
            ),
        }
    }
}

fn invalid_params(msg: &str) -> McpError {
    McpError {
        code: ErrorCode(-32602),
        message: Cow::from(msg.to_string()),
        data: None,
    }
}

fn internal_error(e: crate::error::Error) -> McpError {
    McpError {
        code: ErrorCode(-32603),
        message: Cow::from(e.to_string()),
        data: None,
    }
}
