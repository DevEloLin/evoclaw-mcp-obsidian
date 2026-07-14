# evoclaw-mcp-obsidian

A BM25 knowledge-base **MCP server** over an [Obsidian](https://obsidian.md)
markdown vault, for the [EvoClaw](https://github.com/DevEloLin/evoclaw) agent
runtime.

It gives the agent two tools — `kb_search` and `kb_read` — so it can find and
read relevant notes from your vault by natural-language query, with **Chinese
word segmentation** (jieba), structure awareness (`#tag`, `[[wikilink]]`,
frontmatter), and BM25 ranking.

## No deployment

This is an **stdio** MCP server. EvoClaw spawns it on demand as a child
process and talks to it over stdin/stdout — exactly like the built-in
`filesystem` / `github` servers. There is **no port, no HTTPS, no systemd, no
public endpoint**. You install a binary and drop one TOML file.

```
EvoClaw (MCP client) ──spawn──▶ evoclaw-mcp-obsidian
        └──── stdin/stdout JSON-RPC ────┘
```

## Install

```bash
cd evoclaw-mcp-obsidian
cargo install --path .
```

## Register with EvoClaw

Copy the example and point it at your vault:

```bash
mkdir -p ~/.evoclaw/mcp
cp examples/obsidian.toml ~/.evoclaw/mcp/obsidian.toml
$EDITOR ~/.evoclaw/mcp/obsidian.toml     # set --vault to your vault path
```

```toml
id      = "obsidian"
name    = "Obsidian Knowledge Base"
command = "evoclaw-mcp-obsidian"
args    = ["--vault", "/absolute/path/to/YourVault"]
```

Verify it spawns and lists its tools:

```bash
evoclaw mcp test obsidian
```

The agent now has `mcp__obsidian__kb_search` and `mcp__obsidian__kb_read`.

## Tools

### `kb_search`
BM25 full-text search. Returns a JSON array of ranked hits (`path`, `title`,
`score`, `tags`, `snippet`).

| arg | type | notes |
|-----|------|-------|
| `query` | string (required) | full-text query; Chinese is word-segmented |
| `limit` | int | default from `--max-results`, capped at 25 |
| `tags` | string[] | keep only notes carrying **any** listed tag |
| `path_prefix` | string | keep only notes under this vault-relative prefix |

### `kb_read`
Read a note by vault-relative path. Optional `section` returns just that
heading's slice. Paths are strictly confined to the vault (absolute paths,
`..`, and symlink escapes are rejected).

## How indexing works

On startup the server scans the vault (honoring `--include` / `--exclude`
globs, default `**/*.md`) and builds a [tantivy](https://github.com/quickwit-oss/tantivy)
BM25 index under the OS cache dir (namespaced per vault). Subsequent starts
refresh **incrementally** by file mtime — only changed/new/removed notes are
touched. `title` is boosted ×2 and `headings` ×1.5 in ranking.

> Notes edited *during* a running session are not re-indexed until restart.
> Live file-watching is a planned follow-up.

## CLI

```
evoclaw-mcp-obsidian --vault <PATH> [--exclude <GLOB>]... [--include <GLOB>]...
                     [--max-results <N>] [--cache-dir <PATH>]
```

`--vault` can also be supplied via `EVOCLAW_OBSIDIAN_VAULT`.

## Development

```bash
cargo test        # unit + integration tests
cargo build
```

## License

MIT
