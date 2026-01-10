# WebFetch Tool
## Software Requirements Document
**Version:** 2.0
**Date:** 2026-01-10
**Status:** Draft
**Baseline code reference:** `engine/src/tools/webfetch.rs`, `engine/src/tools/mod.rs`

---

## 0. Change Log

### 0.2 Comprehensive specification update (2026-01-10)
* **Tool naming:** Standardized to `web_fetch` (snake_case), removed alias requirement
* **SSRF:** Added explicit CIDR blocklists, port policy, DNS rebinding mitigation
* **URL normalization:** Added Appendix A with canonical normalization rules
* **robots.txt:** Adopted RFC 9309 semantics, defined origin scoping, failure behavior
* **HTTP:** Added response size limits, content-type handling, redirect semantics
* **Browser mode:** Specified CDP integration, request interception for SSRF
* **Extraction:** Defined deterministic HTML→Markdown algorithm, content-type support matrix
* **Chunking:** Specified heading-aware algorithm, token counting, output size enforcement
* **Caching:** Defined cache key derivation (sha256), TTL, eviction, atomic writes
* **Errors:** Added complete error code registry with structured JSON envelope
* **Config:** Mapped to Forge `ForgeConfig` and `ToolSettings` with precedence rules
* **Tests:** Expanded verification requirements for SSRF, browser mode, determinism

### 0.1 Initial draft (2026-01-08)
* Initial requirements for a WebFetch tool based on `../tools` WebFetch module.

---

## 1. Introduction

### 1.1 Purpose
This document specifies requirements for adding a WebFetch tool to Forge that can retrieve web content safely and present it in an LLM-friendly, token-aware format.

### 1.2 Scope
The WebFetch tool will:
* Validate URLs for SSRF protection
* Respect robots.txt
* Fetch via HTTP with optional headless-browser fallback
* Convert HTML to Markdown
* Chunk content by token budget
* Cache fetch results on disk

Out of scope:
* General web search
* Authentication/login flows
* CAPTCHA bypass
* Arbitrary JS execution beyond headless rendering

### 1.3 Definitions
| Term | Definition |
| --- | --- |
| WebFetch | Forge tool that retrieves a URL and returns Markdown chunks |
| SSRF | Server-Side Request Forgery |
| Chunk | Token-bounded slice of Markdown content |
| Renderer | HTTP fetcher or headless browser used to retrieve content |

### 1.4 References
| Document | Description |
| --- | --- |
| `docs/TOOL_EXECUTOR_SRD.md` | Tool execution framework |
| `../tools/src/webfetch/*` | Reference implementation |
| RFC 2119 / RFC 8174 | Requirement keywords |

### 1.5 Requirement Keywords
The key words **MUST**, **MUST NOT**, **SHALL**, **SHOULD**, **MAY** are as defined in RFC 2119.

---

## 2. Overall Description

### 2.1 Product Perspective
WebFetch runs as a Forge tool through the Tool Executor Framework. It is a networked tool that requires explicit opt-in and is subject to strict security controls.

### 2.2 Product Functions
| Function | Description |
| --- | --- |
| FR-WF-REQ | Accept URL + options and return structured chunks |
| FR-WF-SEC | Enforce SSRF and robots.txt rules |
| FR-WF-REN | Support HTTP and browser rendering |
| FR-WF-EXT | Extract readable content and convert to Markdown |
| FR-WF-CHK | Token-aware chunking with headings |
| FR-WF-CCH | Disk cache with explicit bypass |

### 2.3 User Characteristics
* End users trigger the tool indirectly via LLM tool calls.
* Developers add integrations and adjust policy/config.

### 2.4 Constraints
* Must integrate with Forge tool loop and approval workflow.
* Must not weaken SSRF protections without tests and explicit config.
* Must not exceed tool output limits defined by Tool Executor.

---

## 3. Functional Requirements

### 3.1 Tool Interface

**FR-WF-01 (Canonical name):** Tool name MUST be `web_fetch`. Tool matching MUST be case-sensitive. No aliases are supported. This aligns with Forge's `ToolRegistry` single-name lookup model.

**FR-WF-02 (Request schema):** Request schema MUST include:
* `url` (string, required) — MUST be non-empty, non-whitespace
* `max_chunk_tokens` (integer, optional) — if omitted, use `config.tools.webfetch.default_max_chunk_tokens` (default: 600)
* `no_cache` (boolean, optional, default false)
* `force_browser` (boolean, optional, default false)
* `additionalProperties` MUST be false

**FR-WF-02a (Parameter bounds):** `max_chunk_tokens` MUST be clamped to `[128, 2048]`. Values outside this range MUST return `bad_args` error.

**FR-WF-02b (URL validation):** Empty or whitespace-only `url` MUST return `bad_args` error.

**FR-WF-03 (Response schema):** Response payload MUST be a valid UTF-8 JSON object containing:
* `requested_url` (string) — the original input URL
* `final_url` (string) — the URL after redirect resolution
* `fetched_at` (ISO-8601 timestamp) — original fetch time (from cache metadata on cache hit)
* `title` (optional string) — from `<title>` if present, else first `<h1>`, else omitted
* `language` (optional string) — from `<html lang>` if present and non-empty (BCP-47 tag as-is), else omitted
* `chunks` (array of `FetchChunk`) — see §3.5 for structure
* `rendering_method` ("http" | "browser")
* `truncated` (boolean) — true if output was limited to fit byte budget
* `truncation_reason` (optional string) — e.g., `"tool_output_limit"`, `"max_chunks_reached"`
* `note` (optional string) — e.g., `"cache_hit"`, `"browser_timeout_dom_partial"`, `"robots_unavailable_fail_open"`

**FR-WF-03a (Output size enforcement):** The executor MUST set `ctx.allow_truncation = false` and MUST ensure serialized JSON size is `<= effective_max_bytes`, where `effective_max_bytes = min(ctx.max_output_bytes, ctx.available_capacity_bytes)` per `ToolCtx`. If content exceeds this limit, the tool MUST drop trailing chunks first; if still too large, truncate the final chunk's `text` at a UTF-8 boundary and set `truncated=true`.

**FR-WF-03b (FetchChunk structure):**
```json
{
  "heading": "Section Title",
  "text": "Markdown content including heading line if applicable...",
  "token_count": 450
}
```
* `heading` — the most recent preceding heading text (without `#` prefix), or `""` if none
* `text` — the chunk content (may include the heading line itself)
* `token_count` — token count of `text` only (excluding `heading` field and JSON overhead)

### 3.2 SSRF and URL Validation

#### 3.2.1 URL Parsing and Scheme Validation

**FR-WF-04 (Scheme restriction):** Only `http://` and `https://` schemes are allowed. All other schemes MUST return `invalid_scheme` error.

**FR-WF-04a (URL parsing):** URLs MUST be parsed using a standards-compliant parser (Rust `url` crate, WHATWG URL Standard). Malformed URLs MUST return `invalid_url` error.

**FR-WF-04b (IP literal parsing):** Hostnames parsed as IP literals MUST be validated as IPs. Non-canonical numeric forms (octal `0177.0.0.1`, hex `0x7f000001`, integer `2130706433`) MUST be rejected with `invalid_host` error.

#### 3.2.2 SSRF IP Range Blocking (Normative)

**FR-WF-05 (Blocked CIDR ranges):** SSRF validation MUST reject any destination IP in the following CIDR sets:

**IPv4 blocked ranges:**
| CIDR | Description | Config toggle |
|------|-------------|---------------|
| `127.0.0.0/8` | Loopback | `block_loopback` |
| `10.0.0.0/8` | Private (Class A) | `block_private_ips` |
| `172.16.0.0/12` | Private (Class B) | `block_private_ips` |
| `192.168.0.0/16` | Private (Class C) | `block_private_ips` |
| `169.254.0.0/16` | Link-local | `block_link_local` |
| `0.0.0.0/8` | "This network" | `block_reserved` |
| `100.64.0.0/10` | Carrier-grade NAT | `block_reserved` |
| `192.0.0.0/24` | IETF Protocol Assignments | `block_reserved` |
| `192.0.2.0/24` | Documentation (TEST-NET-1) | `block_reserved` |
| `198.51.100.0/24` | Documentation (TEST-NET-2) | `block_reserved` |
| `203.0.113.0/24` | Documentation (TEST-NET-3) | `block_reserved` |
| `224.0.0.0/4` | Multicast | `block_reserved` |
| `240.0.0.0/4` | Reserved for future use | `block_reserved` |
| `255.255.255.255/32` | Broadcast | `block_reserved` |

**IPv6 blocked ranges:**
| CIDR | Description | Config toggle |
|------|-------------|---------------|
| `::1/128` | Loopback | `block_loopback` |
| `::/128` | Unspecified | `block_reserved` |
| `fc00::/7` | Unique Local Address (ULA) | `block_private_ips` |
| `fe80::/10` | Link-local | `block_link_local` |
| `ff00::/8` | Multicast | `block_reserved` |
| `::ffff:0:0/96` | IPv4-mapped (check mapped IPv4) | (inherit from IPv4) |
| `2001:db8::/32` | Documentation | `block_reserved` |

**FR-WF-05a (IPv4-mapped IPv6):** For `::ffff:0:0/96` addresses, extract the mapped IPv4 and apply IPv4 blocking rules.

**FR-WF-05b (Config override guard):** Disabling any SSRF block via config MUST require `tools.webfetch.security.allow_insecure_overrides = true` AND MUST emit a warning log at startup: `"SSRF protection disabled for: {toggle_names}"`.

#### 3.2.3 Port Policy

**FR-WF-05c (Allowed ports):** Only ports 80 and 443 are allowed by default.

**FR-WF-05d (Port allowlist):** Additional ports MAY be allowed via `tools.webfetch.security.allowed_ports = [8080, 8443]`.

**FR-WF-05e (Port enforcement):** If a URL specifies a port not in the allowlist, the tool MUST return `port_blocked` error.

#### 3.2.4 DNS Resolution and Rebinding Mitigation

**FR-WF-06 (DNS resolution):** Before any HTTP connection, DNS resolution MUST be performed and ALL resolved IPs MUST pass SSRF checks.

**FR-WF-06a (TOCTOU mitigation - HTTP mode):** The implementation MUST use a DNS resolver/connector that pins the resolved IP set during validation. The TCP connection MUST only be made to IPs that passed SSRF validation. This prevents DNS rebinding attacks.

**FR-WF-06b (Resolver trait):** SSRF validation MUST be implemented behind a trait (e.g., `SsrfValidator`) to enable test stubbing:
```rust
pub trait SsrfValidator: Send + Sync {
    fn validate_ip(&self, ip: IpAddr) -> Result<(), SsrfError>;
    fn resolve_and_validate(&self, host: &str) -> Result<Vec<IpAddr>, SsrfError>;
}
```

**FR-WF-06c (DNS failure):** If DNS resolution fails, return `dns_failed` error.

#### 3.2.5 Redirect Handling

**FR-WF-07 (Redirect limit):** Redirects MUST be followed manually and re-validated per hop. Maximum hops MUST default to `max_redirects` (default: 5).

**FR-WF-07a (Redirect status codes):** Follow redirects for HTTP status codes: 301, 302, 303, 307, 308 only.

**FR-WF-07b (Location resolution):** Resolve relative `Location` headers against the current URL per RFC 3986.

**FR-WF-07c (Redirect method):** The tool MUST always use GET (no body) for redirect requests.

**FR-WF-07d (Header preservation):** Preserve `User-Agent` and `Accept` headers across redirects. MUST NOT store or send cookies.

**FR-WF-07e (Per-hop validation):** Each redirect target MUST pass:
1. URL parsing and scheme validation (FR-WF-04)
2. SSRF IP validation after DNS resolution (FR-WF-05, FR-WF-06)
3. robots.txt check (FR-WF-08)

**FR-WF-07f (Redirect loop):** If `max_redirects` is exceeded, return `redirect_limit` error.

### 3.3 robots.txt

#### 3.3.1 Origin Scoping

**FR-WF-08 (Origin-based caching):** robots.txt MUST be fetched and cached per **origin** `(scheme, host, port)`. The fetch URL is `{scheme}://{host}:{port}/robots.txt` (omit port for default 80/443).

**FR-WF-08a (Origin isolation):** `https://example.com` and `http://example.com:8080` are separate origins and MUST NOT share robots.txt state.

#### 3.3.2 Parsing and Evaluation (RFC 9309)

**FR-WF-08b (Parsing algorithm):** robots.txt MUST be parsed according to RFC 9309:
1. Select the most specific user-agent group matching the configured UA token (`tools.webfetch.user_agent`)
2. If no matching group, use the `*` (wildcard) group
3. If no `*` group exists, treat as allow-all

**FR-WF-08c (Path matching precedence):** For a given request path:
1. Collect all `Allow` and `Disallow` rules from the selected group
2. Find the rule with the **longest** matching prefix
3. If equal length matches exist, `Allow` wins over `Disallow`
4. If no rule matches, the path is allowed

**FR-WF-08d (Wildcard support):** The `*` (match any) and `$` (end anchor) patterns in rules MUST be supported per RFC 9309.

**FR-WF-08e (User-agent matching):** User-agent group selection MUST be case-insensitive substring matching of the first token of the configured UA.

#### 3.3.3 Fetch Failure Behavior

**FR-WF-09 (HTTP 404):** If robots.txt returns HTTP 404, treat as allow-all.

**FR-WF-09a (Network/timeout failure):** If robots.txt fetch fails (DNS error, timeout, connection refused, 5xx):
* If `tools.webfetch.robots.fail_open = true` (default: false): treat as allow-all, set `note="robots_unavailable_fail_open"`
* If `tools.webfetch.robots.fail_open = false`: return `robots_unavailable` error

**FR-WF-09b (Malformed robots.txt):** If robots.txt is syntactically invalid, treat as allow-all (permissive parsing).

#### 3.3.4 Caching

**FR-WF-08f (Cache TTL):** robots.txt cache entries MUST have a TTL (default: 24 hours, configurable via `robots_cache_ttl_hours`). Entries MUST be revalidated after TTL expiry.

**FR-WF-08g (Cache eviction):** When `robots_cache_entries` limit is reached, evict entries using LRU (Least Recently Used).

**FR-WF-08h (no_cache interaction):** `no_cache=true` in the request MUST NOT bypass robots.txt enforcement. robots.txt caching operates independently.

### 3.4 Fetch and Rendering

#### 3.4.1 HTTP Mode

**FR-WF-10 (User-agent):** HTTP mode MUST use the configured user-agent string (`tools.webfetch.user_agent`, default: `"forge-webfetch/1.0"`).

**FR-WF-10a (Request timeout):** HTTP mode MUST enforce `timeout_seconds` (default: 20s) as the total request timeout.

**FR-WF-10b (Request headers):** Requests MUST set:
* `User-Agent: {configured_user_agent}`
* `Accept: text/html,application/xhtml+xml,text/plain;q=0.9,*/*;q=0.1`
* `Accept-Encoding: gzip, deflate, br` (if compression supported)

**FR-WF-10c (Compression):** The HTTP client MUST support and automatically decode gzip, deflate, and brotli responses.

**FR-WF-10d (Proxy policy):** The HTTP client MUST disable environment proxy usage (`HTTP_PROXY`, `HTTPS_PROXY`) by default. Proxy usage MAY be enabled via `tools.webfetch.http.use_system_proxy = true`.

**FR-WF-10e (Response size limit):** The tool MUST enforce `max_download_bytes` (default: 5 MiB). If exceeded during download, abort and return `response_too_large` error.

#### 3.4.2 Content-Type Handling

**FR-WF-10f (Supported content types):** Only the following content types are supported:
* `text/html`, `application/xhtml+xml` → HTML extraction pipeline
* `text/plain` → pass-through with minimal normalization

**FR-WF-10g (Unsupported content types):** All other content types (PDF, images, video, `application/json`, etc.) MUST return `unsupported_content_type` error.

**FR-WF-10h (Missing Content-Type):** If `Content-Type` is missing, sniff the first 512 bytes:
* If begins with `<!DOCTYPE` or `<html` (case-insensitive): treat as `text/html`
* Otherwise: treat as `text/plain`

**FR-WF-10i (Charset handling):** Text MUST be decoded to UTF-8:
1. Use charset from `Content-Type` header if present (e.g., `charset=iso-8859-1`)
2. For HTML, check `<meta charset>` or `<meta http-equiv="Content-Type">`
3. Default to UTF-8 with replacement character (U+FFFD) for invalid sequences

#### 3.4.3 Browser Mode

**FR-WF-11 (Browser implementation):** Browser mode MUST be implemented by:
1. Spawning the Chromium binary at `chromium_path` (or from PATH if empty)
2. Using headless mode with DevTools Protocol (CDP) via a CDP client library
3. Minimum Chromium version: 100 (for stable CDP API)

**FR-WF-11a (Browser unavailable):** If Chromium is unavailable and browser mode is required (`force_browser=true`), return `browser_unavailable` error.

**FR-WF-11b (Browser SSRF enforcement - BLOCKING):** Browser mode MUST intercept **all** network requests (document, script, XHR/fetch, iframe, websocket initiation, subresources) and MUST apply SSRF validation (DNS + IP range checks) per-request:
1. Use CDP Network.setRequestInterception or Fetch.enable
2. For each request, resolve DNS and validate all IPs against FR-WF-05
3. Requests that fail SSRF validation MUST be blocked and MUST NOT influence returned DOM

**FR-WF-11c (Browser redirect counting):** Redirects initiated by the browser MUST be counted toward `max_redirects` and revalidated.

**FR-WF-11d (DOM size limit):** Browser mode MUST enforce `max_rendered_dom_bytes` (default: 5 MiB). If the extracted DOM exceeds this, truncate and set `note="browser_dom_truncated"`.

#### 3.4.4 Wait Behavior (Browser Mode)

**FR-WF-11e (Network idle definition):** After navigation completes (DOMContentLoaded):
1. Wait until there are **zero in-flight network requests** for 500ms consecutively
2. Cap total render wait at `network_idle_ms` (default: 20000ms)
3. WebSockets do not count toward "in-flight" for idle detection

**FR-WF-11f (Idle timeout):** If network idle is never reached within `network_idle_ms`, proceed with current DOM and set `note="browser_timeout_dom_partial"`.

#### 3.4.5 Resource Blocking (Browser Mode)

**FR-WF-11g (Resource blocking):** `block_resources` MUST apply to CDP ResourceType values. Default blocked: `Image`, `Media`, `Font`. `Stylesheet` and `Script` MUST NOT be blocked by default.

**FR-WF-11h (Block timing):** Blocking MUST occur before request is issued (via request interception).

#### 3.4.6 Rendering Selection

**FR-WF-12 (HTTP-first strategy):** WebFetch MUST implement HTTP-first rendering with fallback:

**FR-WF-12a (Forced browser):** When `force_browser=true`, skip HTTP mode entirely and use browser mode.

**FR-WF-12b (JS-heavy whitelist):** The whitelist MUST be configuration-driven: `tools.webfetch.rendering.js_heavy_domains = [...]` (default: empty). Domains in this list skip HTTP mode.

**FR-WF-12c (SPA fallback heuristic):** After HTTP extraction, trigger browser fallback when ALL of:
1. Extracted markdown length `< min_extracted_chars` (default: 400)
2. HTML contains SPA indicators: `<script type="module">`, `id="__next"`, `id="app"`, `id="root"`, `window.__NUXT__`, `window.__INITIAL_STATE__`
3. HTTP status was 200

**FR-WF-12d (Browser fallback unavailable):** If browser fallback is selected but browser is unavailable:
* Return HTTP result with `note="browser_unavailable_used_http"`

**FR-WF-12e (Fallback disabled):** SPA fallback MAY be disabled via `tools.webfetch.rendering.spa_fallback_enabled = false`.

### 3.5 Extraction and Chunking

#### 3.5.1 HTML Extraction Algorithm

**FR-WF-13 (Boilerplate removal):** HTML content MUST be processed as follows:
1. Remove elements matching tags: `script`, `style`, `noscript`, `nav`, `footer`, `header`, `aside`
2. Remove elements with `aria-hidden="true"` or `hidden` attribute
3. Remove elements with class containing: `nav`, `menu`, `sidebar`, `footer`, `header`, `advertisement`, `ad-`
4. Extract main content using readability heuristics (prefer `<main>`, `<article>`, `role="main"`)

**FR-WF-13a (Markdown conversion):** Convert cleaned HTML to Markdown:
* Headings: `<h1>`-`<h6>` → `#`-`######`
* Links: `<a href="...">text</a>` → `[text](absolute_url)` — resolve relative URLs per FR-WF-13d
* Images: `<img src="..." alt="...">` → `![alt](absolute_url)` — only if `alt` is non-empty
* Lists: `<ul>/<ol>` → markdown lists with proper nesting
* Code: `<pre><code>` → fenced code blocks; inline `<code>` → backticks
* Tables: `<table>` → markdown tables (best-effort, may simplify complex tables)
* Emphasis: `<em>/<i>` → `*text*`; `<strong>/<b>` → `**text**`

**FR-WF-13b (Whitespace normalization):**
1. Normalize CRLF to LF
2. Collapse runs of `>2` blank lines to exactly 2
3. Trim trailing whitespace from each line
4. Ensure file ends with exactly one newline

**FR-WF-13c (Text/plain handling):** For `text/plain` content, apply only whitespace normalization (FR-WF-13b).

**FR-WF-13d (Link resolution):** All extracted links MUST be normalized to absolute URLs using `final_url` as the base. Fragments MUST be preserved in converted links.

**FR-WF-13e (Title extraction):** `title` field MUST be taken from:
1. `<title>` element if present and non-empty
2. Else first `<h1>` element if present
3. Else omit `title` from response

**FR-WF-13f (Language extraction):** `language` field MUST be taken from `<html lang="...">` if present and non-empty. Value is passed through as-is (BCP-47 format). No language detection is performed.

#### 3.5.2 Token Counting

**FR-WF-14 (Token counter):** Chunk token counts MUST use `forge_context::TokenCounter::count_str()` to ensure consistent behavior across Forge. This uses `cl100k_base` with fallback semantics defined in `context/src/token_counter.rs`.

**FR-WF-14a (Provider variance note):** Token counts are approximate when used with non-OpenAI providers. Different tokenizers produce different counts for the same text. Future versions MAY support provider-specific tokenizers.

#### 3.5.3 Chunking Algorithm

**FR-WF-15 (Block detection):** The input Markdown MUST be split into "blocks":
1. ATX headings (`^#{1,6}\s`) start new blocks
2. Blank-line-separated paragraphs are separate blocks
3. Fenced code blocks (```` ``` ````) are atomic blocks
4. List items at the same level are grouped into a single block

**FR-WF-15a (Chunk accumulation):** Accumulate blocks into chunks:
1. Start with empty chunk, track `current_tokens = 0`
2. For each block, compute `block_tokens = count_tokens(block_text)`
3. If `current_tokens + block_tokens <= max_chunk_tokens`: append block to current chunk
4. Else: emit current chunk, start new chunk with this block

**FR-WF-15b (Oversized block splitting):** If a single block exceeds `max_chunk_tokens`:
1. Split at sentence boundaries (`.!?` followed by space or EOL) if possible
2. Else split at whitespace boundaries
3. Else split at UTF-8 character boundary (never mid-codepoint)
4. Each split piece becomes its own chunk

**FR-WF-15c (Heading tracking):** For each chunk:
* `heading` = text of the most recent preceding ATX heading (without `#` prefix), trimmed
* If no heading precedes the chunk, `heading = ""`
* The heading line itself MUST be included in `text` if it's the first line of the chunk

**FR-WF-15d (token_count field):** `token_count` MUST equal the token count of `chunk.text` only. It excludes the `heading` field value and JSON serialization overhead.

#### 3.5.4 Output Size Enforcement

**FR-WF-15e (Chunk limiting):** After chunking, limit chunks to fit within `effective_max_bytes`:
1. Serialize response JSON incrementally
2. If adding the next chunk would exceed `effective_max_bytes`, stop
3. Set `truncated=true` and `truncation_reason="tool_output_limit"`

**FR-WF-15f (Chunk ordering):** Chunks MUST be returned in document order (first chunk = beginning of content).

### 3.6 Caching

#### 3.6.1 Cache Key Derivation

**FR-WF-16 (Cache key):** Cache key MUST be computed as:
```
cache_key = sha256( canonical_url + "\n" + rendering_method )
```
Where:
* `canonical_url` = normalized URL per Appendix A (FR-WF-NORM)
* `rendering_method` = `"http"` or `"browser"`

**FR-WF-16a (Path layout):** Cache files MUST be stored as:
```
{cache_dir}/{first2}/{keyhex}.json
```
Where `{first2}` is the first two characters of `keyhex` (SHA-256 hex). This prevents large directory listings.

**FR-WF-16b (Safe paths):** No untrusted strings (URL components) may be used directly as path segments. Only the hex-encoded hash is used.

#### 3.6.2 Cache Entry Format

**FR-WF-16c (Metadata storage):** Each cache entry MUST store:
```json
{
  "version": 1,
  "canonical_url": "https://example.com/page",
  "rendering_method": "http",
  "fetched_at": "2026-01-10T12:00:00Z",
  "expires_at": "2026-01-17T12:00:00Z",
  "content": { /* full response payload */ }
}
```

#### 3.6.3 TTL and Eviction

**FR-WF-16d (TTL):** Cache entries MUST have a TTL. Default: 7 days, configurable via `cache_ttl_days`. Entries with `expires_at < now` MUST be treated as cache miss.

**FR-WF-16e (Eviction policy):** The cache MUST enforce `max_cache_entries` (default: 10000) with LRU eviction. On write, if limit is reached, evict least-recently-used entries before writing.

**FR-WF-16f (Size limit):** Optionally enforce `max_cache_bytes` (default: 1 GiB). If set, evict LRU entries until under budget.

#### 3.6.4 Atomicity

**FR-WF-16g (Atomic writes):** Cache writes MUST be atomic:
1. Write to a temp file in the same directory (e.g., `{keyhex}.tmp.{random}`)
2. Rename temp file to final path (POSIX atomic rename)
3. If rename fails (e.g., cross-device), fall back to copy-then-delete

**FR-WF-16h (Write failures):** Cache write failures MUST NOT fail the fetch. On write failure:
* Log warning: `"cache_write_failed: {reason}"`
* Set `note` to include `"cache_write_failed"` if no other note is set

#### 3.6.5 Cache Bypass

**FR-WF-17 (no_cache behavior):** When `no_cache=true`:
1. MUST bypass cache read (treat as cache miss)
2. SHOULD still write fresh result to cache
3. MUST NOT bypass robots.txt enforcement (robots cache is independent)

**FR-WF-17a (Cache hit response):** On cache hit:
* `fetched_at` = original fetch time from cache metadata (not current time)
* `note` includes `"cache_hit"`
* If needed, add `served_at` field with current time

#### 3.6.6 Cache Invalidation

**FR-WF-17b (Manual invalidation):** No automatic cache invalidation beyond TTL. Manual cache clearing is via filesystem operations on `cache_dir`.

### 3.7 Errors and Remediation

#### 3.7.1 Error Encoding

**FR-WF-18 (Error format):** Errors MUST be returned via `ToolError::ExecutionFailed { tool: "web_fetch", message: <json> }`, where `<json>` is a JSON-encoded object:
```json
{
  "code": "ssrf_blocked",
  "message": "Connection to private IP range 10.0.0.0/8 is blocked",
  "retryable": false,
  "details": {
    "blocked_ip": "10.0.0.5",
    "cidr": "10.0.0.0/8"
  }
}
```

**FR-WF-18a (Required fields):**
* `code` (string) — stable error code from registry below
* `message` (string) — human-readable description
* `retryable` (boolean) — whether retry may succeed

**FR-WF-18b (Optional details):** `details` object MAY include error-specific context (blocked IP, HTTP status, etc.).

#### 3.7.2 Error Code Registry (Normative)

| Code | Description | Retryable | Details |
|------|-------------|-----------|---------|
| `bad_args` | Invalid request parameters | No | `field`, `reason` |
| `invalid_url` | URL parsing failed | No | `url` |
| `invalid_scheme` | Non-http(s) scheme | No | `scheme` |
| `invalid_host` | Invalid host (e.g., numeric IP forms) | No | `host` |
| `port_blocked` | Port not in allowlist | No | `port`, `allowed_ports` |
| `ssrf_blocked` | SSRF protection triggered | No | `blocked_ip`, `cidr`, `toggle` |
| `dns_failed` | DNS resolution failed | Yes | `host`, `error` |
| `robots_disallowed` | robots.txt disallows path | No | `path`, `origin` |
| `robots_unavailable` | Could not fetch robots.txt | Yes | `origin`, `error` |
| `redirect_limit` | Max redirects exceeded | No | `count`, `max` |
| `timeout` | Request timeout | Yes | `timeout_ms`, `phase` |
| `network` | Network/connection error | Yes | `error` |
| `response_too_large` | Response exceeds size limit | No | `size`, `max_bytes` |
| `unsupported_content_type` | Content-Type not supported | No | `content_type` |
| `http_4xx` | HTTP 4xx client error | No | `status`, `status_text` |
| `http_5xx` | HTTP 5xx server error | Yes | `status`, `status_text` |
| `browser_unavailable` | Chromium not found/runnable | No | `chromium_path`, `error` |
| `browser_crashed` | Browser process crashed | Yes | `error` |
| `extraction_failed` | HTML extraction failed | No | `error` |
| `cache_read_failed` | Cache read error | Yes | `path`, `error` |
| `internal` | Unexpected internal error | Yes | `error` |

#### 3.7.3 Partial Failure Policy

**FR-WF-19 (No partial results):** Partial downloads/renders MUST NOT be returned. On timeout or error mid-fetch, return the appropriate error code.

**FR-WF-19a (Cache write independence):** Cache write failures MUST NOT fail the fetch — return successful response with `note` indicating cache issue.

#### 3.7.4 Logging

**FR-WF-19b (Log redaction):** Logs MUST redact query strings by default (may contain secrets). Log only scheme, host, and path.

**FR-WF-19c (Structured logging):** Logs MUST include structured fields:
* `event` (e.g., `"fetch_start"`, `"fetch_complete"`, `"ssrf_blocked"`)
* `requested_host`
* `rendering_method`
* `cache_hit` (boolean)
* `error_code` (if error)

---

## 4. Non-Functional Requirements

### 4.1 Security

| Requirement | Specification |
|-------------|---------------|
| NFR-WF-SEC-01 | SSRF protection MUST validate scheme, host, DNS resolution, port, and every redirect hop (FR-WF-04 through FR-WF-07) |
| NFR-WF-SEC-02 | robots.txt MUST be enforced per RFC 9309 for the configured user-agent (FR-WF-08 through FR-WF-09) |
| NFR-WF-SEC-03 | Output MUST be treated as untrusted input — no raw HTML passed to consumers |
| NFR-WF-SEC-04 | Browser mode MUST intercept all subrequests for SSRF validation (FR-WF-11b) |
| NFR-WF-SEC-05 | DNS rebinding MUST be mitigated via pinned resolver (FR-WF-06a) |
| NFR-WF-SEC-06 | Query strings MUST be redacted in logs (FR-WF-19b) |

### 4.2 Performance

| Requirement | Specification |
|-------------|---------------|
| NFR-WF-PERF-01 | HTTP fetches SHOULD complete under `timeout_seconds` (default 20s) |
| NFR-WF-PERF-02 | Chunking SHOULD be O(n) in content length |
| NFR-WF-PERF-03 | Cache lookup SHOULD be O(1) via hash-based key |
| NFR-WF-PERF-04 | robots.txt cache prevents redundant network requests for same origin |

### 4.3 Reliability

| Requirement | Specification |
|-------------|---------------|
| NFR-WF-REL-01 | Cache reads/writes MUST be atomic (temp+rename pattern, FR-WF-16g) |
| NFR-WF-REL-02 | Failures MUST NOT crash the tool loop — errors returned via ToolError |
| NFR-WF-REL-03 | Partial downloads MUST NOT be returned (FR-WF-19) |
| NFR-WF-REL-04 | Cache write failures MUST NOT fail the fetch (FR-WF-16h) |

### 4.4 Approval and Side Effects

| Requirement | Specification |
|-------------|---------------|
| NFR-WF-OP-01 | `is_side_effecting()` MUST return `true` (network egress) |
| NFR-WF-OP-02 | `requires_approval()` MUST return `true` unless `tools.webfetch.allow_auto_execution = true` |
| NFR-WF-OP-03 | `approval_summary()` MUST show scheme/host/path only (redact query strings) and rendering method |

---

## 5. Configuration

### 5.1 Forge Integration

**FR-WF-CFG-01 (Config struct):** Add `webfetch: Option<WebFetchConfig>` to `ToolsConfig` in `engine/src/config.rs`:
```rust
#[derive(Debug, Deserialize, Default)]
pub struct WebFetchConfig {
    pub enabled: bool,
    pub user_agent: Option<String>,
    pub timeout_seconds: Option<u32>,
    pub max_redirects: Option<u32>,
    pub default_max_chunk_tokens: Option<u32>,
    pub cache_dir: Option<String>,
    pub cache_ttl_days: Option<u32>,
    pub max_cache_entries: Option<u32>,
    pub max_download_bytes: Option<u64>,
    pub robots_cache_entries: Option<u32>,
    pub robots_cache_ttl_hours: Option<u32>,
    pub allow_auto_execution: Option<bool>,
    pub browser: Option<WebFetchBrowserConfig>,
    pub security: Option<WebFetchSecurityConfig>,
    pub http: Option<WebFetchHttpConfig>,
    pub rendering: Option<WebFetchRenderingConfig>,
    pub robots: Option<WebFetchRobotsConfig>,
}
```

**FR-WF-CFG-02 (Tool settings):** Extend `tools::ToolSettings` to carry `WebFetchSettings` for registration.

**FR-WF-CFG-03 (Env var expansion):** Apply `config::expand_env_vars()` to `cache_dir` and `chromium_path`.

### 5.2 Precedence Rules

**FR-WF-CFG-PREC-01:** Global `tools.mode=disabled` MUST disable WebFetch regardless of `tools.webfetch.enabled`.

**FR-WF-CFG-PREC-02:** If `tools.mode=enabled` and `tools.webfetch.enabled=true`, WebFetch MUST be registered in `ToolRegistry`.

**FR-WF-CFG-PREC-03:** If `tools.mode=parse_only`, WebFetch MUST NOT execute but tool definition MAY be advertised.

### 5.3 Configuration Reference

```toml
[tools.webfetch]
enabled = false                      # Enable WebFetch tool (default: false)
user_agent = "forge-webfetch/1.0"    # User-Agent header (default: "forge-webfetch/1.0")
timeout_seconds = 20                 # Request timeout (default: 20, range: 1-300)
max_redirects = 5                    # Max redirect hops (default: 5, range: 0-20)
default_max_chunk_tokens = 600       # Default chunk size (default: 600, range: 128-2048)
cache_dir = "${TEMP}/forge-webfetch" # Cache directory (env vars expanded)
cache_ttl_days = 7                   # Cache entry TTL (default: 7)
max_cache_entries = 10000            # Max cached entries (default: 10000)
max_download_bytes = 5242880         # Max response size in bytes (default: 5 MiB)
robots_cache_entries = 1024          # Max robots.txt cache entries (default: 1024)
robots_cache_ttl_hours = 24          # robots.txt cache TTL (default: 24)
allow_auto_execution = false         # Skip approval prompt (default: false, use with caution)

[tools.webfetch.http]
use_system_proxy = false             # Use HTTP_PROXY/HTTPS_PROXY (default: false)

[tools.webfetch.browser]
enabled = true                       # Enable browser mode/fallback (default: true)
chromium_path = ""                   # Path to Chromium binary (empty = search PATH)
network_idle_ms = 20000              # Max wait for network idle (default: 20000)
max_rendered_dom_bytes = 5242880     # Max DOM size (default: 5 MiB)
block_resources = ["image", "font", "media"]  # CDP ResourceTypes to block

[tools.webfetch.security]
block_private_ips = true             # Block RFC 1918 ranges (default: true)
block_loopback = true                # Block 127.0.0.0/8, ::1 (default: true)
block_link_local = true              # Block 169.254.0.0/16, fe80::/10 (default: true)
block_reserved = true                # Block other reserved ranges (default: true)
allowed_ports = [80, 443]            # Allowed destination ports (default: [80, 443])
allow_insecure_overrides = false     # Required to disable SSRF protections (default: false)

[tools.webfetch.rendering]
js_heavy_domains = []                # Domains that skip HTTP mode (default: [])
spa_fallback_enabled = true          # Enable SPA detection fallback (default: true)
min_extracted_chars = 400            # Threshold for SPA fallback (default: 400)

[tools.webfetch.robots]
fail_open = false                    # Allow fetch if robots.txt unavailable (default: false)
```

### 5.4 Configuration Validation

| Field | Type | Default | Range | Notes |
|-------|------|---------|-------|-------|
| `timeout_seconds` | u32 | 20 | 1-300 | Clamped to range |
| `max_redirects` | u32 | 5 | 0-20 | Clamped to range |
| `default_max_chunk_tokens` | u32 | 600 | 128-2048 | Clamped to range |
| `max_download_bytes` | u64 | 5242880 | 1024-104857600 | 1 KiB to 100 MiB |
| `allowed_ports` | Vec<u16> | [80, 443] | Valid port numbers | Empty = default |

---

## 6. Verification Requirements

### 6.1 Unit Tests - SSRF

| Test ID | Description | Requirement |
|---------|-------------|-------------|
| T-WF-SSRF-01 | Reject `ftp://`, `file://`, `javascript:` schemes | FR-WF-04 |
| T-WF-SSRF-02 | Reject `localhost`, `127.0.0.1`, `::1` | FR-WF-05 |
| T-WF-SSRF-03 | Reject `10.x.x.x`, `172.16.x.x`, `192.168.x.x` | FR-WF-05 |
| T-WF-SSRF-04 | Reject `169.254.x.x` (link-local) | FR-WF-05 |
| T-WF-SSRF-05 | Reject `::ffff:10.0.0.1` (IPv4-mapped) | FR-WF-05a |
| T-WF-SSRF-06 | Reject octal/hex/integer IP forms | FR-WF-04b |
| T-WF-SSRF-07 | Reject non-allowed ports | FR-WF-05c |
| T-WF-SSRF-08 | Accept custom ports when configured | FR-WF-05d |
| T-WF-SSRF-09 | Reject redirect to private IP (via wiremock) | FR-WF-07e |
| T-WF-SSRF-10 | DNS rebinding: stubbed resolver returns blocked IP | FR-WF-06a |
| T-WF-SSRF-11 | Require `allow_insecure_overrides` to disable protections | FR-WF-05b |

### 6.2 Unit Tests - robots.txt

| Test ID | Description | Requirement |
|---------|-------------|-------------|
| T-WF-ROB-01 | Disallow path per robots.txt | FR-WF-08c |
| T-WF-ROB-02 | Allow path when not in Disallow | FR-WF-08c |
| T-WF-ROB-03 | Allow beats Disallow on equal-length match | FR-WF-08c |
| T-WF-ROB-04 | Longest match wins | FR-WF-08c |
| T-WF-ROB-05 | Fall back to `*` user-agent group | FR-WF-08b |
| T-WF-ROB-06 | 404 robots.txt = allow all | FR-WF-09 |
| T-WF-ROB-07 | Timeout with fail_open=false returns error | FR-WF-09a |
| T-WF-ROB-08 | Timeout with fail_open=true allows with note | FR-WF-09a |
| T-WF-ROB-09 | Per-origin isolation (different ports/schemes) | FR-WF-08a |
| T-WF-ROB-10 | Wildcard `*` and `$` patterns | FR-WF-08d |

### 6.3 Unit Tests - Extraction and Chunking

| Test ID | Description | Requirement |
|---------|-------------|-------------|
| T-WF-EXT-01 | Boilerplate elements removed | FR-WF-13 |
| T-WF-EXT-02 | Links normalized to absolute URLs | FR-WF-13d |
| T-WF-EXT-03 | Whitespace normalized (CRLF→LF, blank lines collapsed) | FR-WF-13b |
| T-WF-EXT-04 | Title extracted from `<title>` or `<h1>` | FR-WF-13e |
| T-WF-CHK-01 | Chunk sizes do not exceed max_chunk_tokens | FR-WF-15a |
| T-WF-CHK-02 | Oversized blocks split at sentence boundaries | FR-WF-15b |
| T-WF-CHK-03 | Heading tracked across chunks | FR-WF-15c |
| T-WF-CHK-04 | token_count matches text only | FR-WF-15d |
| T-WF-CHK-05 | Output fits effective_max_bytes | FR-WF-15e |

### 6.4 Unit Tests - Caching

| Test ID | Description | Requirement |
|---------|-------------|-------------|
| T-WF-CCH-01 | Cache hit returns prior content with correct fetched_at | FR-WF-17a |
| T-WF-CCH-02 | no_cache=true bypasses read, still writes | FR-WF-17 |
| T-WF-CCH-03 | Expired entries treated as miss | FR-WF-16d |
| T-WF-CCH-04 | HTTP vs browser renders have separate cache keys | FR-WF-16 |
| T-WF-CCH-05 | Cache path uses hash, no URL components | FR-WF-16b |
| T-WF-CCH-06 | LRU eviction when limit reached | FR-WF-16e |

### 6.5 Unit Tests - Errors

| Test ID | Description | Requirement |
|---------|-------------|-------------|
| T-WF-ERR-01 | Error response is valid JSON with code/message/retryable | FR-WF-18 |
| T-WF-ERR-02 | All error codes in registry are produced by code paths | FR-WF-18 registry |
| T-WF-ERR-03 | Query strings redacted in logs | FR-WF-19b |

### 6.6 Integration Tests

| Test ID | Description | Requirement |
|---------|-------------|-------------|
| IT-WF-HTTP-01 | Fetch and extract Markdown via HTTP (wiremock) | FR-WF-10, FR-WF-13 |
| IT-WF-HTTP-02 | Follow redirects, validate each hop | FR-WF-07 |
| IT-WF-HTTP-03 | Unsupported content-type returns error | FR-WF-10g |
| IT-WF-HTTP-04 | Response size limit enforced | FR-WF-10e |
| IT-WF-HTTP-05 | Charset detection (ISO-8859-1 → UTF-8) | FR-WF-10i |

### 6.7 Integration Tests - Browser Mode

**T-WF-BR-ENV-01:** Browser integration tests MUST be skipped unless `FORGE_TEST_CHROMIUM_PATH` is set.

| Test ID | Description | Requirement |
|---------|-------------|-------------|
| IT-WF-BR-01 | Browser render succeeds when forced | FR-WF-11, FR-WF-12a |
| IT-WF-BR-02 | Browser-unavailable returns error | FR-WF-11a |
| IT-WF-BR-03 | Browser SSRF: page JS fetches private IP, blocked | FR-WF-11b |
| IT-WF-BR-04 | Browser SSRF: XHR to localhost, blocked | FR-WF-11b |
| IT-WF-BR-05 | Browser timeout produces partial DOM with note | FR-WF-11f |
| IT-WF-BR-06 | Resource blocking prevents image fetches | FR-WF-11g |
| IT-WF-BR-07 | SPA fallback triggered for minimal content | FR-WF-12c |

### 6.8 Test Infrastructure Requirements

**T-WF-INFRA-01:** SSRF tests MUST use a stubbed `SsrfValidator` trait implementation, not system DNS.

**T-WF-INFRA-02:** HTTP tests MUST use wiremock for deterministic server responses.

**T-WF-INFRA-03:** Cache tests MUST use tempfile directories for isolation.

**T-WF-INFRA-04:** Browser SSRF test MUST serve a page via wiremock that includes JS attempting to fetch internal IP, then verify the internal content is NOT in returned DOM.

### 6.9 Determinism Tests

| Test ID | Description | Requirement |
|---------|-------------|-------------|
| T-WF-DET-01 | Same input URL produces same cache key | FR-WF-16 |
| T-WF-DET-02 | URL normalization is deterministic | FR-WF-NORM |
| T-WF-DET-03 | Chunking is deterministic for same content | FR-WF-15 |

---

## Appendix A: URL Normalization (Normative)

This appendix defines the canonical URL normalization used for cache keys, robots.txt origin matching, and deterministic behavior.

### A.1 Normalization Steps

**FR-WF-NORM-01 (Fragment removal):** Remove the URL fragment (`#...`) before validation, fetch, robots checks, and caching.

**FR-WF-NORM-02 (Case normalization):** Normalize scheme and host to lowercase.

**FR-WF-NORM-03 (Default port elision):** Remove default ports from the URL:
* `:80` for `http://`
* `:443` for `https://`

**FR-WF-NORM-04 (Path normalization):**
1. Resolve `.` and `..` path segments per RFC 3986
2. Ensure path starts with `/` (add if missing)
3. Do NOT remove trailing slashes (they are semantically significant)

**FR-WF-NORM-05 (Query preservation):** Do NOT reorder query parameters. Preserve original order.

**FR-WF-NORM-06 (Percent-encoding):** Normalize percent-encoding:
* Uppercase hex digits (`%2f` → `%2F`)
* Decode unreserved characters (A-Z, a-z, 0-9, `-`, `.`, `_`, `~`)
* Do NOT decode reserved characters

### A.2 Canonical URL String

The canonical URL is: `{scheme}://{host}[:{port}]{path}[?{query}]`

Where:
* `scheme` = lowercase
* `host` = lowercase (or IPv6 in brackets, lowercase hex)
* `port` = omitted if default, else decimal
* `path` = normalized path starting with `/`
* `query` = original query string (if present)

### A.3 Examples

| Input | Canonical |
|-------|-----------|
| `HTTPS://Example.COM/Page#section` | `https://example.com/Page` |
| `http://example.com:80/` | `http://example.com/` |
| `https://example.com:443/a/../b` | `https://example.com/b` |
| `http://EXAMPLE.com:8080/` | `http://example.com:8080/` |
| `https://example.com/path?b=2&a=1` | `https://example.com/path?b=2&a=1` |

---

## Appendix B: WebFetch State Machine

This appendix describes the processing pipeline as a state machine for clarity.

### B.1 State Diagram

```
┌─────────────┐
│   START     │
└──────┬──────┘
       │ url input
       ▼
┌─────────────┐     invalid
│  ParseURL   │────────────────► ERROR(invalid_url)
└──────┬──────┘
       │ valid
       ▼
┌─────────────┐     blocked
│ NormalizeURL│────────────────► ERROR(invalid_scheme|invalid_host)
└──────┬──────┘
       │ normalized
       ▼
┌─────────────┐     cache hit
│ CheckCache  │────────────────► RETURN(cached_response)
└──────┬──────┘
       │ cache miss
       ▼
┌─────────────┐     blocked
│  SSRFCheck  │────────────────► ERROR(ssrf_blocked|port_blocked)
│  (DNS+IP)   │
└──────┬──────┘
       │ allowed
       ▼
┌─────────────┐     blocked
│ RobotsCheck │────────────────► ERROR(robots_disallowed|robots_unavailable)
└──────┬──────┘
       │ allowed
       ▼
┌─────────────┐     error
│   Fetch     │────────────────► ERROR(timeout|network|http_4xx|http_5xx|...)
│ (HTTP/Brwsr)│
└──────┬──────┘
       │ success
       ▼
┌─────────────┐     redirect
│ CheckStatus │────────────────► (loop: SSRFCheck → RobotsCheck → Fetch)
└──────┬──────┘
       │ 200 OK
       ▼
┌─────────────┐     unsupported
│ContentType  │────────────────► ERROR(unsupported_content_type)
└──────┬──────┘
       │ html/text
       ▼
┌─────────────┐     error
│  Extract    │────────────────► ERROR(extraction_failed)
└──────┬──────┘
       │ markdown
       ▼
┌─────────────┐
│   Chunk     │
└──────┬──────┘
       │ chunks[]
       ▼
┌─────────────┐
│ FitToOutput │ (drop trailing chunks if needed)
└──────┬──────┘
       │ fitted
       ▼
┌─────────────┐
│ WriteCache  │ (best-effort)
└──────┬──────┘
       │
       ▼
┌─────────────┐
│   RETURN    │
└─────────────┘
```

### B.2 Redirect Loop

On redirect (3xx status):
1. Increment redirect counter
2. If counter > `max_redirects`: ERROR(redirect_limit)
3. Parse Location header → new URL
4. Loop back to SSRFCheck with new URL
5. Each hop validates SSRF + robots independently

### B.3 Rendering Selection (within Fetch)

```
force_browser=true ──────────────► Browser Mode
        │ false
        ▼
domain in js_heavy_domains ──────► Browser Mode
        │ no
        ▼
HTTP Mode ───────► Extract ───────► check extracted_chars < min?
                                            │ yes AND SPA indicators
                                            ▼
                                    Browser Fallback (if available)
```

---

## Appendix C: References

| Document | Description |
|----------|-------------|
| RFC 2119 / RFC 8174 | Requirement keywords (MUST, SHOULD, MAY) |
| RFC 3986 | URI Generic Syntax |
| RFC 9309 | robots.txt specification |
| WHATWG URL Standard | URL parsing specification |
| `engine/src/tools/mod.rs` | Forge ToolExecutor trait |
| `engine/src/config.rs` | Forge configuration |
| `context/src/token_counter.rs` | Token counting implementation |

---

## Appendix D: Glossary

| Term | Definition |
|------|------------|
| CDP | Chrome DevTools Protocol — interface for controlling Chromium |
| CIDR | Classless Inter-Domain Routing — IP address range notation |
| LRU | Least Recently Used — cache eviction strategy |
| SSRF | Server-Side Request Forgery — attack where server makes unintended requests |
| SPA | Single Page Application — JS-heavy sites that render client-side |
| TOCTOU | Time-of-Check to Time-of-Use — race condition vulnerability |
| TTL | Time To Live — cache entry expiration duration |
| UA | User-Agent — HTTP header identifying the client |

