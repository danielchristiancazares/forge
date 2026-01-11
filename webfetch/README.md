# forge-webfetch

WebFetch is the Forge tool that retrieves web content safely and returns
LLM-friendly Markdown chunks. It implements SSRF protection, robots.txt checks,
HTTP fetching with optional headless browser rendering, HTML-to-Markdown
extraction, token-aware chunking, and disk caching.

## Usage

```rust
use forge_webfetch::{fetch, WebFetchConfig, WebFetchInput};

# async fn example() -> Result<(), forge_webfetch::WebFetchError> {
let input = WebFetchInput::new("https://example.com")?;
let config = WebFetchConfig::default();
let output = fetch(input, &config).await?;
# Ok(())
# }
```

## Configuration

`WebFetchConfig` maps to `[tools.webfetch]` in Forge config. Key fields:

- `enabled`: enable/disable the tool.
- `user_agent`, `timeout_seconds`, `max_redirects`, `max_download_bytes`.
- `browser`: headless Chromium settings (see `BrowserConfig`).
- `robots`: robots.txt behavior (see `RobotsConfig`).
- `security`: SSRF policy (see `SecurityConfig`).
- `cache_dir`, `cache_ttl_days`, `max_cache_entries`, `max_cache_bytes`.

For detailed behavior, see `docs/WEBFETCH_SRD.md`.

## Testing

```sh
cargo test -p forge-webfetch
```

Browser integration tests are skipped unless
`FORGE_TEST_CHROMIUM_PATH` is set to a Chromium/Chrome executable.

## Invariants

Core logic operates on `resolved::ResolvedConfig` and `ResolvedRequest` so
optional configuration is handled at the edges, keeping invariants explicit
and reducing `Option` usage in the hot path.
