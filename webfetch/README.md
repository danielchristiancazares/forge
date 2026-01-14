# forge-webfetch

WebFetch is the Forge tool that retrieves web content safely and returns
LLM-friendly Markdown chunks. It implements SSRF protection, robots.txt
compliance (RFC 9309), HTTP fetching with optional headless browser rendering,
HTML-to-Markdown extraction, token-aware chunking, and LRU disk caching.

## Architecture

The crate is organized into these modules:

| Module | Purpose |
|--------|---------|
| `types` | Domain types: input, output, configuration, and errors |
| `http` | HTTP client with SSRF validation and DNS pinning |
| `browser` | CDP-based headless Chromium rendering (optional) |
| `robots` | RFC 9309 robots.txt parser and checker with caching |
| `extract` | HTML to Markdown extraction with boilerplate removal |
| `chunk` | Token-aware content chunking with heading tracking |
| `cache` | LRU disk cache with TTL and dual-limit eviction |
| `resolved` | Invariant-safe configuration resolution |

## Key Types

### Input Types

**`WebFetchInput`**: Request parameters for a fetch operation.

```rust
pub struct WebFetchInput {
    url: Url,                       // Parsed and validated URL
    original_url: String,           // Original URL string for output
    max_chunk_tokens: Option<u32>,  // Token budget per chunk [128, 2048]
    no_cache: bool,                 // Bypass cache if true
    force_browser: bool,            // Force browser rendering
}
```

Builder pattern with validation:

```rust
let input = WebFetchInput::new("https://example.com")?
    .with_max_chunk_tokens(800)?   // Validates range [128, 2048]
    .with_no_cache(true)
    .with_force_browser(false);
```

### Output Types

**`WebFetchOutput`**: Successful fetch result.

```rust
pub struct WebFetchOutput {
    requested_url: String,              // Original input URL (unchanged)
    final_url: String,                  // Final URL after redirects (fragment removed)
    fetched_at: String,                 // RFC 3339 timestamp
    title: Option<String>,              // Page title from <title> or <h1>
    language: Option<String>,           // Language from <html lang>
    chunks: Vec<FetchChunk>,            // Token-bounded content chunks
    rendering_method: RenderingMethod,  // Http or Browser
    truncated: bool,                    // Whether content was truncated
    truncation_reason: Option<TruncationReason>,
    notes: Vec<Note>,                   // Condition tokens (cache_hit, etc.)
}
```

**`FetchChunk`**: A single chunk of extracted content.

```rust
pub struct FetchChunk {
    heading: String,     // Most recent preceding heading (no # prefix)
    text: String,        // Markdown content
    token_count: u32,    // Token count of text field only
}
```

**`Note`**: Condition tokens reported in the output (in canonical order):

| Note | Description |
|------|-------------|
| `CacheHit` | Response served from cache |
| `RobotsUnavailableFailOpen` | robots.txt unavailable, proceeded anyway |
| `BrowserUnavailableUsedHttp` | Browser requested but unavailable |
| `BrowserDomTruncated` | DOM exceeded max_rendered_dom_bytes |
| `BrowserBlockedNonGet` | Non-GET/HEAD subrequests were blocked |
| `CharsetFallback` | Unknown charset; used UTF-8 with replacement |
| `CacheWriteFailed` | Cache write failed (fetch still succeeded) |
| `ToolOutputLimit` | Output truncated to fit byte budget |

### Error Types

**`WebFetchError`**: Structured error with stable code, message, and retryability.

```rust
pub struct WebFetchError {
    code: ErrorCode,            // Stable error code
    message: String,            // Human-readable description
    retryable: bool,            // Whether retry may succeed
    details: ErrorDetails,      // Key-value context pairs
}
```

**`ErrorCode`** variants:

| Code | Retryable | Description |
|------|-----------|-------------|
| `BadArgs` | No | Invalid request parameters |
| `InvalidUrl` | No | URL parsing failed |
| `InvalidScheme` | No | Non-http(s) scheme |
| `InvalidHost` | No | Invalid host (e.g., non-canonical IP) |
| `PortBlocked` | No | Port not in allowlist |
| `SsrfBlocked` | No | SSRF protection triggered |
| `DnsFailed` | Yes | DNS resolution failed |
| `RobotsDisallowed` | No | robots.txt disallows path |
| `RobotsUnavailable` | Yes | Could not fetch robots.txt |
| `RedirectLimit` | No | Max redirects exceeded |
| `Timeout` | Yes | Request timeout |
| `Network` | Yes | Network/connection error |
| `ResponseTooLarge` | No | Response exceeds size limit |
| `UnsupportedContentType` | No | Content-Type not supported |
| `Http4xx` | Conditional | HTTP 4xx (retryable for 408/429) |
| `Http5xx` | Yes | HTTP 5xx server error |
| `BrowserUnavailable` | No | Chromium not found/runnable |
| `BrowserCrashed` | Yes | Browser process crashed |
| `ExtractionFailed` | No | HTML extraction failed |
| `Internal` | Yes | Unexpected internal error |

## Public API

The primary entry point is the `fetch` function:

```rust
pub async fn fetch(
    input: WebFetchInput,
    config: &WebFetchConfig,
) -> Result<WebFetchOutput, WebFetchError>
```

### Fetch Pipeline

1. **Cache Check**: Look up cached content (unless `no_cache`)
2. **SSRF Validation**: Validate URL scheme, host, port, and resolve DNS
3. **robots.txt Check**: Verify path is allowed for our user-agent
4. **Content Fetch**: HTTP request with optional browser fallback
5. **Extraction**: Convert HTML to Markdown with boilerplate removal
6. **Chunking**: Split content by token budget with heading tracking
7. **Cache Write**: Store result for future requests

## How It Works

### SSRF Protection

The HTTP module implements comprehensive SSRF protection:

- **Scheme validation**: Only `http` and `https` allowed
- **Userinfo rejection**: No credentials in URLs
- **IPv6 zone identifier rejection**: Blocks `%` in bracketed hosts
- **Non-canonical IP detection**: Rejects octal/hex IP forms
- **Port allowlist**: Default ports 80, 443 (configurable)
- **CIDR blocking**: Default blocks for private/reserved ranges:
  - `127.0.0.0/8`, `10.0.0.0/8`, `172.16.0.0/12`, `192.168.0.0/16`
  - `169.254.0.0/16`, `::1/128`, `fc00::/7`, `fe80::/10`, etc.
- **DNS pinning**: Resolved IPs pinned to prevent rebinding attacks
- **Redirect validation**: Each redirect hop is SSRF-checked

### robots.txt Compliance

RFC 9309 compliant parser with:

- **User-agent matching**: Case-insensitive substring match; most specific wins
- **Rule evaluation**: Longest matching rule wins; Allow beats Disallow on ties
- **Pattern support**: Path prefix, `*` wildcards, `$` end anchors
- **Empty rules**: Empty Disallow = allow-all; Empty Allow = matches nothing
- **Origin caching**: In-memory cache with configurable TTL and entry limit
- **Fail-open option**: Proceed on robots.txt fetch failure (adds note)
- **Redirect handling**: Allows http to https upgrade, same host only

### Browser Rendering

Optional headless Chromium rendering via CDP (Chrome DevTools Protocol):

- **Isolated profiles**: Temporary user-data-dir per session (auto-cleaned)
- **Request interception**: All subrequests SSRF-validated via Fetch API
- **Method restrictions**: Only GET/HEAD allowed for subrequests
- **Resource blocking**: Configurable blocking (images, fonts, media)
- **Subresource budget**: Limits total bytes from subrequests
- **DOM size limit**: Truncates if rendered DOM exceeds limit
- **Network idle detection**: Waits for 500ms of network quiet
- **SPA detection**: Auto-fallback to browser for minimal HTML shells

Chromium is discovered via:
1. Explicit path in config
2. `PATH` environment search
3. Platform-specific default locations

### HTML Extraction

Content extraction with intelligent boilerplate removal:

- **Root detection cascade**: `<main>`, `<article>`, `[role="main"]`, `#content`, `.content`, `<body>`
- **Tag-level filtering**: Removes `<script>`, `<style>`, `<nav>`, `<footer>`, `<aside>`, etc.
- **Attribute filtering**: Removes `aria-hidden="true"`, `hidden`, `role="navigation"`
- **Token-based class/ID filtering**: Removes elements with boilerplate tokens (nav, menu, sidebar, etc.)
- **Minimum content check**: Requires 50+ non-whitespace characters

Markdown conversion supports:
- Headings (h1-h6)
- Paragraphs, blockquotes
- Ordered and unordered lists (nested)
- Code blocks with language hints
- Links with URL resolution
- Images (only if alt text present)
- Tables (GFM pipe format)
- Inline formatting (bold, italic, strikethrough)
- Definition lists
- Figures with captions

### Token-Aware Chunking

Content is split respecting structure and token budgets:

- **Block-based splitting**: Paragraphs, code fences, lists treated as units
- **Heading state machine**: Tracks current heading for chunk context
- **Code block atomicity**: Keeps fenced code blocks together when possible
- **Overflow handling**: Sentence > whitespace > character boundaries
- **List splitting**: Splits at item boundaries, preserves continuations
- **tiktoken counting**: Accurate token counts via tiktoken-rs

### Disk Caching

LRU cache with dual-limit eviction:

- **Key derivation**: SHA256 of `canonical_url + "\n" + rendering_method`
- **Path layout**: `{cache_dir}/{first2hex}/{keyhex}.json`
- **Versioned entries**: Format version check; stale versions deleted
- **TTL expiration**: Configurable days; expired entries treated as miss
- **Dual limits**: Evicts by entry count AND total bytes (whichever exceeded first)
- **LRU tracking**: `last_accessed_at` updated on read (no TTL sliding)
- **Atomic writes**: Temp file + rename for crash safety
- **Re-chunking**: Cached markdown re-chunked with request's token budget

## Configuration

`WebFetchConfig` maps to `[tools.webfetch]` in Forge config:

```toml
[tools.webfetch]
enabled = true                      # Enable/disable the tool
user_agent = "forge-webfetch/1.0"   # HTTP User-Agent string
timeout_seconds = 20                # Request timeout
max_redirects = 5                   # Maximum redirect hops
max_download_bytes = 10485760       # 10 MiB download limit
default_max_chunk_tokens = 600      # Default token budget per chunk
max_dns_attempts = 3                # DNS resolution retries

# Cache settings
cache_dir = "~/.cache/forge/webfetch"  # Cache directory
cache_ttl_days = 7                     # Cache entry lifetime
max_cache_entries = 1000               # Maximum cached URLs
max_cache_bytes = 1073741824           # 1 GiB total cache size

# robots.txt settings
robots_cache_entries = 1024            # Origins cached in memory
robots_cache_ttl_hours = 24            # robots.txt cache lifetime

[tools.webfetch.security]
blocked_cidrs = []                  # Additional blocked CIDRs
allowed_ports = [80, 443]           # Allowed destination ports
allow_insecure_tls = false          # Dangerous: skip TLS verification
allow_insecure_overrides = false    # Bypass default SSRF blocks

[tools.webfetch.http]
headers = []                        # Additional request headers
use_system_proxy = false            # Use HTTP_PROXY/HTTPS_PROXY
connect_timeout_seconds = 10        # TCP connection timeout
read_timeout_seconds = 30           # Response read timeout

[tools.webfetch.robots]
user_agent_token = "forge-webfetch" # Token for robots.txt matching
fail_open = false                   # Proceed if robots.txt unavailable

[tools.webfetch.browser]
enabled = true                      # Enable browser fallback
chromium_path = ""                  # Explicit Chromium path (or search PATH)
timeout_seconds = 20                # Browser navigation timeout
max_rendered_dom_bytes = 5242880    # 5 MiB DOM limit
max_subresource_bytes = 20971520    # 20 MiB subresource limit
blocked_resource_types = ["image", "font", "media"]
```

## Usage Examples

### Basic Fetch

```rust
use forge_webfetch::{fetch, WebFetchConfig, WebFetchInput};

async fn example() -> Result<(), forge_webfetch::WebFetchError> {
    let input = WebFetchInput::new("https://example.com")?;
    let config = WebFetchConfig::default();
    let output = fetch(input, &config).await?;

    println!("Title: {:?}", output.title);
    println!("Chunks: {}", output.chunks.len());
    for chunk in &output.chunks {
        println!("[{}] {} tokens", chunk.heading, chunk.token_count);
    }
    Ok(())
}
```

### Custom Token Budget

```rust
let input = WebFetchInput::new("https://docs.rs")?
    .with_max_chunk_tokens(1024)?;  // Larger chunks
```

### Force Browser Rendering

```rust
let input = WebFetchInput::new("https://spa-app.example.com")?
    .with_force_browser(true);
```

### Bypass Cache

```rust
let input = WebFetchInput::new("https://news.example.com")?
    .with_no_cache(true);
```

### Handle Errors

```rust
match fetch(input, &config).await {
    Ok(output) => {
        if output.notes.contains(&Note::CacheHit) {
            println!("Served from cache");
        }
    }
    Err(e) => {
        eprintln!("Error [{}]: {}", e.code, e.message);
        if e.retryable {
            // Consider retry with backoff
        }
    }
}
```

## Integration with Other Crates

- **`forge-context`**: Uses `TokenCounter` for accurate token counting

## Testing

```sh
cargo test -p forge-webfetch
```

Browser integration tests are skipped unless `FORGE_TEST_CHROMIUM_PATH` is set
to a Chromium/Chrome executable.

```sh
FORGE_TEST_CHROMIUM_PATH="/usr/bin/chromium" cargo test -p forge-webfetch
```

## Design Principles

### Invariant-First Architecture

Core logic operates on `resolved::ResolvedConfig` and `ResolvedRequest` types.
Optional configuration is resolved at the boundary, keeping invariants explicit
and eliminating `Option` handling in the hot path.

### Layered Error Handling

- **Structured errors**: Stable codes, human messages, retryability hints
- **Condition notes**: Non-fatal conditions reported in output (not exceptions)
- **Cache failures**: Non-fatal; fetch proceeds, note added to output

### Security by Default

- SSRF protection enabled with sensible defaults
- robots.txt compliance required (fail-closed by default)
- Browser requests fully intercepted and validated

## Specification

For complete behavioral specification, see `docs/WEBFETCH_SRD.md`.
