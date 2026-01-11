# WebFetch Tool
## Software Requirements Document
**Version:** 2.1
**Date:** 2026-01-10
**Status:** Implementation-Ready
**Baseline code reference:** `engine/src/tools/webfetch.rs`, `engine/src/tools/mod.rs`

## LLM-TOC
<!-- Auto-generated section map for LLM context -->
| Lines | Section |
|-------|---------|
| 1-90 | Header, Change Log, Section 1 - Introduction: purpose, scope, definitions |
| 91-130 | Section 2 - Overall Description: product perspective, functions, constraints |
| 131-200 | Section 3.1 - Tool Interface: request schema, response schema, output enforcement |
| 201-300 | Section 3.2.1-3.2.4 - SSRF Validation: URL parsing, IP blocklists, port policy, DNS rebinding |
| 301-380 | Section 3.2.5-3.3 - Redirects and robots.txt: redirect handling, RFC 9309 parsing |
| 381-500 | Section 3.4 - Fetch and Rendering: HTTP mode, Content-Type, browser mode, CDP/SSRF architecture |
| 501-660 | Section 3.5 - Extraction and Chunking: HTML algorithm, deterministic extraction, chunking |
| 661-760 | Section 3.6 - Caching: cache key derivation, LRU tracking, TTL, eviction, entry format |
| 761-850 | Section 3.7 and 4 - Error Handling and NFRs: error codes, structured JSON passthrough |
| 851-950 | Section 5 - Configuration: Forge integration, precedence rules, reference |
| 951-1020 | Section 6 - Verification: SSRF tests, robots.txt tests, extraction tests, browser tests |
| 1021-1072 | Appendices A-E: URL normalization, state machine, references, glossary, executor integration |

---

## 0. Change Log

### 0.3 Implementation-readiness remediation (2026-01-10)
* **Status:** Upgraded from Draft to Implementation-Ready after GPT-5.2p review
* **Parameter bounds (A1):** `max_chunk_tokens` now rejects (not clamps) out-of-range values
* **Output enforcement (B1-B4):** Added canonical algorithm FR-WF-OUT-01; clarified `ctx.allow_truncation` ownership
* **Notes array (C1-C2):** Changed `note` → `notes` array with defined tokens; added URL field semantics
* **Error handling (D1-D6):** Added FR-ERR-JSON-01/02 for structured error passthrough; schema validation errors now return JSON envelope
* **Timeout ownership (E1):** Added FR-WF-TIMEOUT-01 ensuring tool timeout exceeds internal budgets
* **URL validation (E2-E4):** Added IDNA/punycode, userinfo rejection, IPv6 zone identifier rejection, IP literal test vectors
* **Port policy (F1):** Clarified allowlist semantics (override, not additive)
* **DNS pinning (F2):** Added FR-WF-DNS-01 with deterministic connection strategy
* **Security overrides (F3):** Added FR-WF-SEC-OVR-01 requiring explicit `allow_insecure_overrides`
* **robots.txt (F4-F7):** Added UA specificity, rule length precedence, query string inclusion, redirect handling
* **Content-Type (G1-G2):** Added binary magic sniffing, charset fallback handling
* **Timeout budgeting (G3):** Added FR-WF-TIMEOUT-02 for redirect chain budget
* **Browser SSRF (G4-G7):** Added FR-WF-BSSR-01 architecture (CDP Fetch.fulfillRequest or proxy); DOM size measurement; resource type matching
* **Extraction (H1-H4):** Added deterministic class/id token matching, root selection order, table conversion rules, code fence whitespace preservation
* **Chunking (I1-I3):** Added list block detection, code block atomicity, heading state machine
* **Caching (J1-J4):** Clarified cache key inputs, LRU tracking mechanism, no_cache write behavior, entry format versioning
* **Tests (L1-L2):** Added browser SSRF test harness requirements, golden test stability guidance
* **Appendix E:** Added Tool Executor integration documentation

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

**FR-WF-02a (Parameter bounds):** `max_chunk_tokens` MUST be an integer in `[128, 2048]`. Values outside this range MUST return `bad_args` error. The tool MUST NOT clamp out-of-range values—explicit rejection ensures the caller is aware of invalid input.

**FR-WF-02b (URL validation):** Empty or whitespace-only `url` MUST return `bad_args` error.

**FR-WF-03 (Response schema):** Response payload MUST be a valid UTF-8 JSON object containing:
* `requested_url` (string) — the original input URL as provided (unchanged)
* `final_url` (string) — the canonicalized URL (per Appendix A) of the last fetched URL with fragment removed
* `fetched_at` (ISO-8601 timestamp) — original fetch time (from cache metadata on cache hit)
* `title` (optional string) — from `<title>` if present, else first `<h1>`, else omitted
* `language` (optional string) — from `<html lang>` if present and non-empty (BCP-47 tag as-is), else omitted
* `chunks` (array of `FetchChunk`) — see §3.5 for structure
* `rendering_method` ("http" | "browser")
* `truncated` (boolean) — true if output was limited to fit byte budget
* `truncation_reason` (optional string) — e.g., `"tool_output_limit"`, `"max_chunks_reached"`
* `notes` (array of strings, default `[]`) — stable tokens indicating conditions that occurred during fetch

**FR-WF-RESP-URL-01 (URL field semantics):** `requested_url` MUST equal the original `args.url` string exactly as provided by the caller (no normalization). `final_url` MUST be the canonicalized URL (Appendix A) of the last fetched URL **with fragment removed**. If no redirects occurred, `final_url` is the canonicalized form of `requested_url`.

**FR-WF-03c (Notes array):** The `notes` array replaces the singular `note` field. Each condition MUST append a stable token; ordering MUST reflect occurrence order during processing. Defined tokens:
| Token | Condition |
|-------|-----------|
| `cache_hit` | Response served from cache |
| `cache_write_failed` | Cache write failed (fetch still succeeded) |
| `robots_unavailable_fail_open` | robots.txt unavailable but `fail_open=true` |
| `browser_timeout_dom_partial` | Browser render timed out; partial DOM extracted |
| `browser_dom_truncated` | DOM exceeded `max_rendered_dom_bytes` |
| `browser_unavailable_used_http` | Browser fallback requested but unavailable |
| `charset_fallback` | Unknown charset; fell back to UTF-8 with replacement |

**FR-WF-03a (Output size enforcement):** The **WebFetch tool's `execute()` method** MUST set `ctx.allow_truncation = false` before returning output, preventing the framework's generic truncation marker (`"... [output truncated]"`) from invalidating the JSON response. The tool MUST ensure serialized JSON size is `<= effective_max_bytes`, where `effective_max_bytes = min(ctx.max_output_bytes, ctx.available_capacity_bytes)` per `ToolCtx`.

**FR-WF-OUT-01 (Canonical output enforcement algorithm):** The tool MUST enforce `effective_max_bytes` using this deterministic algorithm:
1. Build all chunks per the chunking algorithm (§3.5.3).
2. Serialize the full response JSON. If within `effective_max_bytes`, return.
3. If over limit, drop chunks from the end one-by-one until payload fits OR only one chunk remains. Set `truncated=true` and `truncation_reason="tool_output_limit"`.
4. If still over limit with exactly one chunk, truncate that chunk's `text` at a UTF-8 boundary until payload fits.
5. After truncation, `token_count` MUST be recomputed for the final chunk text.

This algorithm supersedes any conflicting descriptions in §3.5.4.

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

**FR-WF-04b (IP literal parsing):** Hostnames parsed as IP literals MUST be validated as IPs. Non-canonical numeric forms MUST be rejected with `invalid_host` error. Forbidden forms:
* Single-integer (dword) form: `2130706433`
* Hex form with `0x` prefix: `0x7f000001`
* Octal form with leading zeros: `0177.0.0.1`
* Mixed-base dotted forms: `0x7f.0.0.1`

**Test vectors (all MUST be rejected):**
| Input | Reason |
|-------|--------|
| `http://2130706433/` | Dword IP (127.0.0.1) |
| `http://0x7f000001/` | Hex IP (127.0.0.1) |
| `http://0177.0.0.1/` | Octal IP (127.0.0.1) |
| `http://0x7f.0.0.1/` | Mixed-base IP |
| `http://017700000001/` | Octal dword |

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

**FR-WF-SEC-OVR-01 (Invalid override rejection):** If any SSRF block is disabled (e.g., `block_private_ips=false`) while `allow_insecure_overrides != true`, the application MUST refuse to start with a configuration error:
```
Configuration error: SSRF protection cannot be disabled without allow_insecure_overrides=true
Affected settings: block_private_ips=false
```
This prevents silent insecurity from misconfiguration.

#### 3.2.3 Port Policy

**FR-WF-05c (Allowed ports):** Only ports 80 and 443 are allowed by default.

**FR-WF-PORT-01 (Port allowlist semantics):** `allowed_ports` is the **complete allowlist** (override, not additive). Default is `[80, 443]`. If configured to a non-empty list, **only those ports** are permitted—the default ports are NOT implicitly included.
* Example: `allowed_ports = [8080]` means ONLY port 8080 is allowed; ports 80/443 are blocked
* Example: `allowed_ports = [80, 443, 8080]` allows all three
* Empty list (`[]`) means "use default `[80, 443]`"

**FR-WF-05d (Port allowlist config):** Additional ports are allowed by setting the complete list: `tools.webfetch.security.allowed_ports = [80, 443, 8080, 8443]`.

**FR-WF-05e (Port enforcement):** If a URL specifies a port not in the allowlist, the tool MUST return `port_blocked` error.

#### 3.2.4 DNS Resolution and Rebinding Mitigation

**FR-WF-06 (DNS resolution):** Before any HTTP connection, DNS resolution MUST be performed and ALL resolved IPs MUST pass SSRF checks.

**FR-WF-06a (TOCTOU mitigation - HTTP mode):** The implementation MUST use a DNS resolver/connector that pins the resolved IP set during validation. The TCP connection MUST only be made to IPs that passed SSRF validation. This prevents DNS rebinding attacks.

**FR-WF-DNS-01 (Deterministic connection strategy):** The resolver MUST return an ordered list of IPs. Connection behavior:
1. Attempt connection in resolver-returned order (typically AAAA before A for dual-stack)
2. Try at most `max_dns_attempts` addresses (default: 2) before failing
3. Re-resolution during a fetch is **forbidden**—use only the initially resolved set
4. If all attempted addresses fail, return `network` error with details

This ensures deterministic behavior across implementations when DNS returns multiple addresses.

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

**FR-WF-ROBOTS-UA-01 (User-agent specificity):** A group matches if any `User-agent` line contains the UA token as a case-insensitive substring. Among matching groups, choose the group with the **longest** matching `User-agent` value; tie-break by file order (first wins).
* Example: UA token `forge`, groups `User-agent: forge-webfetch` and `User-agent: forge` → select `forge-webfetch` (longer match)

**FR-WF-08c (Path matching precedence):** For a given request path:
1. Collect all `Allow` and `Disallow` rules from the selected group
2. Find the rule with the **longest** matching prefix
3. If equal length matches exist, `Allow` wins over `Disallow`
4. If no rule matches, the path is allowed

**FR-WF-ROBOTS-RULE-01 (Rule length with wildcards):** Rule precedence MUST be determined by the length of the rule's pattern string (excluding the directive name), counting all characters including `*` and `$`. Longer length wins; on tie, `Allow` wins.
* Example: `Disallow: /private/*` (11 chars) vs `Allow: /private/public/` (16 chars) → `Allow` wins for `/private/public/x`

**FR-WF-08d (Wildcard support):** The `*` (match any) and `$` (end anchor) patterns in rules MUST be supported per RFC 9309.

**FR-WF-ROBOTS-PATH-01 (Query string inclusion):** The robots matching input MUST be `path` plus `?query` if present, excluding fragment. This aligns with RFC 9309 which specifies matching against the URL-path.
* Example: `Disallow: /*?session=` blocks URLs with `?session=` in the query string

**FR-WF-08e (User-agent matching):** User-agent group selection MUST be case-insensitive substring matching of the first token of the configured UA.

**FR-WF-ROBOTS-FETCH-01 (Redirect handling):** robots.txt retrieval MUST follow redirects up to `max_redirects`, applying the same SSRF validation and port policy per hop as document fetching. A redirect from `http://` to `https://` for robots.txt is common and MUST be followed.

#### 3.3.3 Fetch Failure Behavior

**FR-WF-09 (HTTP 404):** If robots.txt returns HTTP 404, treat as allow-all.

**FR-WF-09a (Network/timeout failure):** If robots.txt fetch fails (DNS error, timeout, connection refused, 5xx):
* If `tools.webfetch.robots.fail_open = true` (default: false): treat as allow-all, append `"robots_unavailable_fail_open"` to `notes`
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

**FR-WF-TIMEOUT-02 (Redirect chain budgeting):** `timeout_seconds` is the budget for the **entire HTTP-mode fetch** including all redirect hops, download, and decoding. Each hop shares this single budget—there is no per-hop timeout reset.

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
1. If contains NUL byte (`\x00`) or matches known binary magic: return `unsupported_content_type` error
2. If begins with `<!DOCTYPE` or `<html` (case-insensitive): treat as `text/html`
3. Otherwise: treat as `text/plain`

**Binary magic signatures to detect:**
| Bytes | Type |
|-------|------|
| `%PDF-` | PDF |
| `\x89PNG` | PNG image |
| `GIF87a` or `GIF89a` | GIF image |
| `\xFF\xD8\xFF` | JPEG image |
| `PK\x03\x04` | ZIP/Office document |
| `\x00\x00\x00\x1C ftypmp4` (or similar) | MP4 video |

**FR-WF-10i (Charset handling):** Text MUST be decoded to UTF-8:
1. Use charset from `Content-Type` header if present (e.g., `charset=iso-8859-1`)
2. For HTML, check `<meta charset>` or `<meta http-equiv="Content-Type">`
3. Default to UTF-8 with replacement character (U+FFFD) for invalid sequences

**FR-WF-CHARSET-01 (Charset normalization):** Charset names MUST be matched case-insensitively. Supported charsets: `UTF-8`, `ISO-8859-1` (Latin-1), `Windows-1252`. Unknown charsets MUST fall back to UTF-8 with replacement and append `"charset_fallback"` to `notes`.

#### 3.4.3 Browser Mode

**FR-WF-11 (Browser implementation):** Browser mode MUST be implemented by:
1. Spawning the Chromium binary at `chromium_path` (or from PATH if empty)
2. Using headless mode with DevTools Protocol (CDP) via a CDP client library
3. Minimum Chromium version: 100 (for stable CDP API)

**FR-WF-11a (Browser unavailable):** If Chromium is unavailable and browser mode is required (`force_browser=true`), return `browser_unavailable` error.

**FR-WF-11b (Browser SSRF enforcement - BLOCKING):** Browser mode MUST intercept **all** network requests (document, script, XHR/fetch, iframe, websocket initiation, subresources) and MUST apply SSRF validation (DNS + IP range checks) per-request.

**FR-WF-BSSR-01 (Browser SSRF architecture):** To prevent TOCTOU/DNS rebinding attacks where the browser's native networking resolves a different IP than validated, the implementation MUST use one of these architectures:

**Option A - CDP Fetch.fulfillRequest (Recommended):**
1. Enable CDP `Fetch.enable` with pattern matching all requests
2. For each `Fetch.requestPaused` event:
   a. Resolve DNS using the tool's DNS resolver
   b. Validate all resolved IPs against FR-WF-05
   c. If valid: use the tool's HTTP client to fetch the resource with IP pinning, then call `Fetch.fulfillRequest` with the response
   d. If invalid: call `Fetch.failRequest` with `BlockedByClient`
3. This ensures the tool controls all DNS resolution and TCP connections

**Option B - Local Proxy:**
1. Spawn a local HTTP(S) proxy that performs SSRF validation + IP pinning
2. Configure the browser to use this proxy for all requests via `--proxy-server`
3. The proxy validates each request before forwarding

"Allowing" a request to proceed via the browser's native networking without IP pinning is **forbidden**—this would permit DNS rebinding attacks.

**FR-WF-11c (Browser redirect counting):** Redirects initiated by the browser for the **main document navigation** MUST be counted toward `max_redirects` and revalidated.

**FR-WF-BREDIR-01 (Redirect scope):** `max_redirects` in browser mode applies only to the **main document navigation chain** (top frame). Subresource redirects (images, scripts, XHR) do NOT count toward this limit but MUST still pass SSRF validation per hop.

**FR-WF-11d (DOM size limit):** Browser mode MUST enforce `max_rendered_dom_bytes` (default: 5 MiB). If the extracted DOM exceeds this, truncate and append `"browser_dom_truncated"` to `notes`.

**FR-WF-DOMSIZE-01 (DOM size measurement):** DOM size is measured as the UTF-8 byte length of `document.documentElement.outerHTML` at extraction time. If over limit, the tool MUST abort further waiting and proceed to extraction immediately.

#### 3.4.4 Wait Behavior (Browser Mode)

**FR-WF-11e (Network idle definition):** After navigation completes (DOMContentLoaded):
1. Wait until there are **zero in-flight network requests** for 500ms consecutively
2. Cap total render wait at `network_idle_ms` (default: 20000ms)
3. WebSockets do not count toward "in-flight" for idle detection

**FR-WF-11f (Idle timeout):** If network idle is never reached within `network_idle_ms`, proceed with current DOM and append `"browser_timeout_dom_partial"` to `notes`. This partial DOM extraction is permitted and does not constitute an error (see FR-WF-19).

#### 3.4.5 Resource Blocking (Browser Mode)

**FR-WF-11g (Resource blocking):** `block_resources` MUST apply to CDP ResourceType values. Default blocked: `Image`, `Media`, `Font`. `Stylesheet` and `Script` MUST NOT be blocked by default.

**FR-WF-BLOCKRES-01 (Resource type matching):** `block_resources` entries MUST be matched **case-insensitively** against CDP resource type names (e.g., `"image"` matches `Image`, `"IMAGE"` matches `Image`). Unknown entries MUST cause `bad_args` error at tool invocation time.

Valid CDP ResourceType values: `Document`, `Stylesheet`, `Image`, `Media`, `Font`, `Script`, `TextTrack`, `XHR`, `Fetch`, `Prefetch`, `EventSource`, `WebSocket`, `Manifest`, `SignedExchange`, `Ping`, `CSPViolationReport`, `Preflight`, `Other`.

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
* Return HTTP result with `"browser_unavailable_used_http"` appended to `notes`

**FR-WF-12e (Fallback disabled):** SPA fallback MAY be disabled via `tools.webfetch.rendering.spa_fallback_enabled = false`.

### 3.5 Extraction and Chunking

#### 3.5.1 HTML Extraction Algorithm

**FR-WF-13 (Boilerplate removal):** HTML content MUST be processed as follows:
1. Remove elements matching tags: `script`, `style`, `noscript`, `nav`, `footer`, `header`, `aside`
2. Remove elements with `aria-hidden="true"` or `hidden` attribute
3. Remove elements with class/id matching boilerplate tokens (see FR-WF-EXT-CLASS-01)
4. Extract main content using deterministic root selection (see FR-WF-EXT-ROOT-01)

**FR-WF-EXT-CLASS-01 (Class/id matching):** Boilerplate matching MUST use **case-insensitive token matching** (space-separated class tokens or id value), NOT substring matching. Boilerplate tokens:
| Token | Removes |
|-------|---------|
| `nav` | Navigation elements |
| `menu` | Menu elements |
| `sidebar` | Sidebar elements |
| `footer` | Footer elements |
| `header` | Header elements |
| `advertisement` | Ad containers |
| `ad` | Ad containers |
| `social` | Social sharing widgets |
| `related` | Related content sections |
| `comments` | Comment sections |

**Matching rules:**
* Split `class` attribute on whitespace; match if ANY token equals a boilerplate token (case-insensitive)
* Match `id` attribute if it equals a boilerplate token (case-insensitive)
* **Substring matching is forbidden** — `class="navigate"` does NOT match `nav`, but `class="site-nav"` DOES match `nav` only if tokenized (it doesn't—`site-nav` is a single token)

**FR-WF-EXT-ROOT-01 (Extraction root selection):** Choose the extraction root in this deterministic order:
1. First `<main>` element
2. Else first `<article>` element
3. Else first element with `role="main"`
4. Else first element with `id="content"` (case-insensitive)
5. Else first element with `class` containing token `content` (case-insensitive)
6. Else `<body>`

If the chosen root is empty after boilerplate removal, fall back to the next option in order.

**FR-WF-13a (Markdown conversion):** Convert cleaned HTML to Markdown:
* Headings: `<h1>`-`<h6>` → `#`-`######`
* Links: `<a href="...">text</a>` → `[text](absolute_url)` — resolve relative URLs per FR-WF-13d
* Images: `<img src="..." alt="...">` → `![alt](absolute_url)` — only if `alt` is non-empty
* Lists: `<ul>/<ol>` → markdown lists with proper nesting (see FR-WF-EXT-LIST-01)
* Code: `<pre><code>` → fenced code blocks; inline `<code>` → backticks (see FR-WF-EXT-CODE-01)
* Tables: `<table>` → GitHub-flavored pipe tables (see FR-WF-EXT-TABLE-01)
* Emphasis: `<em>/<i>` → `*text*`; `<strong>/<b>` → `**text**`

**FR-WF-EXT-TABLE-01 (Table conversion):** Tables MUST be converted to GitHub-flavored Markdown pipe tables:
1. `rowspan` and `colspan` attributes MUST be ignored — each `<td>/<th>` becomes exactly one cell
2. Use the first `<tr>` with `<th>` elements (or first `<tr>` if no `<th>`) as the header row
3. Add separator row with `|---|` pattern matching column count
4. Cells containing newlines MUST have newlines replaced with `<br>` or space
5. Pipe characters (`|`) within cell text MUST be escaped as `\|`
6. If table has no usable header row, synthesize empty headers: `| | | |`

**FR-WF-EXT-LIST-01 (List nesting):** List nesting level MUST be determined by counting ancestor `<ul>` and `<ol>` elements:
* Level 0: no list ancestors → no indent
* Level 1: one list ancestor → 2 spaces indent
* Level N: N list ancestors → N×2 spaces indent
This ensures deterministic indentation regardless of source HTML whitespace.

**FR-WF-EXT-CODE-01 (Code fence preservation):** Fenced code blocks (`<pre><code>`) MUST preserve internal whitespace exactly:
1. Detect language hint from `class="language-xxx"` on `<code>` element
2. Extract text content preserving all whitespace (including leading/trailing)
3. Output as: ` ``` ` + language + newline + content + newline + ` ``` `
4. If content contains ` ``` `, use ` ```` ` as fence (increase fence length until unique)

**FR-WF-13b (Whitespace normalization):**
1. Normalize CRLF to LF
2. Collapse runs of `>2` blank lines to exactly 2
3. Trim trailing whitespace from each line
4. Ensure file ends with exactly one newline

**FR-WF-EXT-WS-01 (Code fence exemption):** Whitespace normalization (steps 2-3 above) MUST NOT modify content between fenced code block delimiters (` ``` `). Only CRLF→LF normalization (step 1) applies inside code fences. This preserves semantically significant whitespace in code samples.

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
3. Fenced code blocks (` ``` `) are atomic blocks (see FR-WF-CHK-CODE-01)
4. List blocks: consecutive list items form a single block (see FR-WF-CHK-LIST-01)

**FR-WF-CHK-LIST-01 (List block detection):** A "list block" is a contiguous sequence of lines that:
1. Start with list markers: `- `, `* `, `+ `, or `1. ` (digits followed by `.` or `)`)
2. Include continuation lines (indented content belonging to list items)
3. End at: a blank line followed by non-list content, OR a heading, OR end of document

List items at different nesting levels are part of the SAME list block if contiguous. The entire nested structure forms one atomic block for chunking purposes.

**FR-WF-CHK-CODE-01 (Code block atomicity):** Fenced code blocks are atomic:
1. A code block starts at ` ``` ` (or longer fence) and ends at the matching closing fence
2. Code blocks MUST NOT be split mid-block during chunking
3. If a code block alone exceeds `max_chunk_tokens`, it becomes a single oversized chunk (FR-WF-15b applies for splitting)
4. When splitting an oversized code block, split at line boundaries only (preserve complete lines)

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

**FR-WF-CHK-HEAD-01 (Heading state machine):** Heading tracking MUST use a state machine:
1. Initialize `current_heading = ""`
2. For each block in document order:
   a. If block starts with ATX heading (`^#{1,6}\s+(.+)$`), extract heading text (group 1), trim whitespace, set `current_heading`
   b. When emitting a chunk, record `heading = current_heading` at the moment of emission
3. Heading level is NOT tracked—only the most recent heading text regardless of level

**FR-WF-CHK-HEAD-02 (Heading in text):** When a chunk's first block is a heading:
1. The heading line (including `#` prefix) MUST appear as the first line of `chunk.text`
2. The `heading` field MUST equal the same heading's text (without `#` prefix)
3. This means the heading appears in both `heading` and `text` for such chunks

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

**FR-WF-CCH-KEY-01 (Cache key inputs):** The cache key computation uses EXACTLY these inputs:
1. `canonical_url`: The **final** URL after all redirects, normalized per Appendix A (fragment removed)
2. `rendering_method`: Literal string `"http"` or `"browser"` (not the config value, but the actual method used)

Request parameters (`max_chunk_tokens`, `no_cache`, `force_browser`) are NOT part of the cache key. This means:
* Same URL fetched with different `max_chunk_tokens` values share a cache entry
* Chunking is recomputed on cache hit if `max_chunk_tokens` differs from cached result

**FR-WF-CCH-KEY-02 (Hash algorithm):** SHA-256 MUST be used. The hash input is UTF-8 encoded. Output is lowercase hexadecimal (64 characters).

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

**FR-WF-CCH-VER-01 (Entry format versioning):** The `version` field enables forward compatibility:
1. Current version: `1`
2. On read, if `version > SUPPORTED_VERSION`, treat as cache miss (don't attempt to parse)
3. On read, if `version < CURRENT_VERSION`, migrate or treat as cache miss (implementation choice)
4. Version increments when `content` schema changes incompatibly

**FR-WF-CCH-CONTENT-01 (Content field):** The `content` field stores the **complete response payload** as returned to the caller, including all `chunks`, `title`, `language`, `rendering_method`, etc. On cache hit, `content` is returned directly (with `fetched_at` from metadata, not from `content`).

#### 3.6.3 TTL and Eviction

**FR-WF-16d (TTL):** Cache entries MUST have a TTL. Default: 7 days, configurable via `cache_ttl_days`. Entries with `expires_at < now` MUST be treated as cache miss.

**FR-WF-16e (Eviction policy):** The cache MUST enforce `max_cache_entries` (default: 10000) with LRU eviction. On write, if limit is reached, evict least-recently-used entries before writing.

**FR-WF-CCH-LRU-01 (LRU tracking):** "Recently used" is determined by the `last_accessed` timestamp in cache metadata:
1. On cache **read** (hit): update `last_accessed` to current time (touch the file or update metadata)
2. On cache **write**: set `last_accessed` to current time
3. Eviction selects entries with the oldest `last_accessed` timestamp

Implementation options:
* **File mtime**: Use filesystem modification time as `last_accessed` (update on read via `touch`)
* **Metadata field**: Store `last_accessed` in the JSON entry and rewrite on access

The file mtime approach is RECOMMENDED for simplicity and atomicity.

**FR-WF-16f (Size limit):** Optionally enforce `max_cache_bytes` (default: 1 GiB). If set, evict LRU entries until under budget.

#### 3.6.4 Atomicity

**FR-WF-16g (Atomic writes):** Cache writes MUST be atomic:
1. Write to a temp file in the same directory (e.g., `{keyhex}.tmp.{random}`)
2. Rename temp file to final path (POSIX atomic rename)
3. If rename fails (e.g., cross-device), fall back to copy-then-delete

**FR-WF-16h (Write failures):** Cache write failures MUST NOT fail the fetch. On write failure:
* Log warning: `"cache_write_failed: {reason}"`
* Append `"cache_write_failed"` to `notes`

#### 3.6.5 Cache Bypass

**FR-WF-17 (no_cache behavior):** When `no_cache=true`:
1. MUST bypass cache read (treat as cache miss)
2. MUST still write fresh result to cache (overwriting any existing entry)
3. MUST NOT bypass robots.txt enforcement (robots cache is independent)

**FR-WF-CCH-NOCACHE-01 (no_cache write behavior):** `no_cache=true` means "don't read from cache" but DOES write:
* The fresh fetch result overwrites any existing cache entry for the same key
* This allows `no_cache` to be used for cache refresh/warming
* To completely avoid cache interaction, implementers must disable caching at config level

**FR-WF-17a (Cache hit response):** On cache hit:
* `fetched_at` = original fetch time from cache metadata (not current time)
* Append `"cache_hit"` to `notes`
* If needed, add `served_at` field with current time

#### 3.6.6 Cache Invalidation

**FR-WF-17b (Manual invalidation):** No automatic cache invalidation beyond TTL. Manual cache clearing is via filesystem operations on `cache_dir`.

### 3.7 Errors and Remediation

#### 3.7.1 Error Encoding

**FR-WF-18 (Error format):** Errors MUST be returned via `ToolError::ExecutionFailed { tool: "web_fetch", message: <json> }`, where `<json>` is a JSON-encoded object.

**FR-ERR-JSON-01 (JSON passthrough requirement - Tool Executor integration):** The Tool Executor framework (see Appendix E) MUST detect when `ToolError::ExecutionFailed.message` begins with `{` and parses as a valid JSON object. In this case, the engine MUST pass the JSON through to `ToolResult::error` **without prefixing** (i.e., no `"{tool} failed: "` wrapper). This preserves structured error codes for model consumption.

**FR-ERR-JSON-02 (Schema validation errors):** When argument schema validation fails for tool `"web_fetch"` (via `tools::validate_args` or equivalent), the engine MUST emit `ToolResult::error` with the JSON envelope:
```json
{ "code": "bad_args", "message": "<schema error description>", "retryable": false, "details": { "validation": "<optional details>" } }
```
This ensures schema validation failures are also returned in the structured format rather than a plain string.

**FR-WF-TIMEOUT-01 (Timeout ownership):** WebFetch MUST implement `ToolExecutor::timeout()` and return a duration that exceeds all internal operation budgets:
```
executor_timeout = timeout_seconds + (network_idle_ms / 1000) + page_load_buffer_s + 2s
```
Where `page_load_buffer_s` defaults to 5s. This ensures the tool's internal timeout fires first, allowing the tool to return a structured `timeout` error rather than the engine's generic `ToolError::Timeout`.

Error envelope structure:
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

**FR-WF-19 (No partial HTTP downloads):** Partial **HTTP downloads** (incomplete body due to connection drop or timeout) MUST NOT be returned. On timeout or error mid-fetch in HTTP mode, return the appropriate error code.

**FR-WF-19-BROWSER (Browser partial DOM permitted):** For **browser rendering**, returning a DOM snapshot before `network_idle_ms` is **permitted** and constitutes a successful response. The tool MUST:
1. Set `truncated=true` if the DOM was extracted before network idle
2. Append `"browser_timeout_dom_partial"` to `notes`
3. Return the extracted content (not an error)

This distinction exists because browser-rendered SPAs often never reach true network idle, yet the DOM is usable.

**FR-WF-19a (Cache write independence):** Cache write failures MUST NOT fail the fetch — return successful response with `"cache_write_failed"` appended to `notes`.

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
| NFR-WF-OP-02 | `requires_approval()` behavior — see FR-WF-OP-02 below |
| NFR-WF-OP-03 | `approval_summary()` MUST show scheme/host/path only (redact query strings) and rendering method |

**FR-WF-OP-02 (Auto-execution mapping):** If `tools.webfetch.allow_auto_execution=true`, the engine MUST treat `"web_fetch"` as **allowlisted for approval purposes** (equivalent to adding it to `tools.approval.allowlist`). This means:
1. `requires_approval()` returns `false`
2. The tool bypasses approval prompts even though `is_side_effecting()=true`
3. Global deny mode (`tools.approval.mode=deny`) still overrides this setting

This ensures `allow_auto_execution` produces predictable "no prompt" behavior across implementations.

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

**FR-WF-CFG-PREC-03 (Parse-only behavior):** If `tools.mode=parse_only`:
1. WebFetch MUST NOT execute (execution attempts return error)
2. WebFetch tool definition is advertised to the model **only if** explicitly included in `[[tools.definitions]]` config
3. If not in `[[tools.definitions]]`, WebFetch is not advertised (consistent with Forge's parse-only design where tools come from config, not registry)

This aligns with Forge's current behavior where `parse_only` mode uses config-defined tools exclusively.

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

**T-WF-INFRA-05 (Browser SSRF test harness):** The browser SSRF test (IT-WF-BR-03, IT-WF-BR-04) MUST:
1. Start wiremock on a random port (avoid port conflicts)
2. Serve a page containing: `<script>fetch('http://127.0.0.1:{internal_port}/secret').then(r=>r.text()).then(t=>document.body.innerHTML+=t)</script>`
3. Start a second wiremock on `{internal_port}` serving `/secret` → `"LEAKED_INTERNAL_DATA"`
4. Invoke WebFetch with browser mode on the first page
5. Assert: returned markdown does NOT contain `LEAKED_INTERNAL_DATA`
6. Assert: test logs show SSRF block event for `127.0.0.1`

This proves the browser's JS-initiated requests are intercepted and blocked.

**T-WF-INFRA-06 (Golden test stability):** Extraction and chunking tests SHOULD use golden file comparisons:
1. Store expected output as `tests/golden/{test_name}.md`
2. On test run, compare actual output byte-for-byte with golden file
3. Update goldens via `UPDATE_GOLDENS=1 cargo test` (or similar flag)
4. Golden files MUST be committed to version control

Golden tests ensure extraction/chunking changes are intentional and reviewed.

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

**FR-WF-NORM-07 (IDNA/Punycode):** Hosts containing non-ASCII Unicode characters MUST be normalized to ASCII using IDNA (punycode) prior to canonicalization and cache key derivation. Canonical URLs MUST use the ASCII form.
* Example: `https://münich.example/` → `https://xn--mnich-kva.example/`

**FR-WF-URL-USERINFO-01 (Userinfo rejection):** URLs containing a username or password component (e.g., `https://user:pass@example.com/`) MUST be rejected with `invalid_url` error. This prevents credential leakage into logs, caches, and approval summaries.

**FR-WF-IPV6-ZONE-01 (IPv6 zone identifiers):** IPv6 literals containing a zone identifier (percent-encoded `%25`, e.g., `http://[fe80::1%25lo0]/`) MUST be rejected with `invalid_url` error. Zone identifiers are ambiguous across hosts and can bypass naïve IP parsing.

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

---

## Appendix E: Tool Executor Integration

This appendix documents how WebFetch integrates with Forge's Tool Executor framework (`docs/TOOL_EXECUTOR_SRD.md`).

### E.1 ToolExecutor Trait Implementation

WebFetch MUST implement the `ToolExecutor` trait:

```rust
impl ToolExecutor for WebFetchTool {
    fn name(&self) -> &'static str { "web_fetch" }
    
    fn is_side_effecting(&self) -> bool { true }  // Network egress
    
    fn requires_approval(&self, ctx: &ToolCtx) -> bool {
        // Returns false if allow_auto_execution=true AND not in deny mode
        !self.config.allow_auto_execution || ctx.approval_mode == ApprovalMode::Deny
    }
    
    fn timeout(&self) -> Duration {
        // Must exceed internal timeouts (FR-WF-TIMEOUT-01)
        Duration::from_secs(self.timeout_seconds + self.network_idle_ms/1000 + 7)
    }
    
    async fn execute(&self, args: Value, ctx: ToolCtx) -> Result<ToolOutput, ToolError> {
        // Set allow_truncation=false before returning (FR-WF-03a)
        ctx.allow_truncation = false;
        // ... implementation
    }
}
```

### E.2 Error Passthrough Contract

Per FR-ERR-JSON-01, the Tool Executor MUST detect JSON error messages and pass them through without prefixing:

```rust
// In tool_executor.rs or equivalent
fn format_error(tool: &str, error: ToolError) -> ToolResult {
    match error {
        ToolError::ExecutionFailed { message, .. } => {
            // Detect structured JSON error
            if message.starts_with('{') && serde_json::from_str::<Value>(&message).is_ok() {
                // Pass through without prefix
                ToolResult::error(message)
            } else {
                // Legacy: prefix with tool name
                ToolResult::error(format!("{tool} failed: {message}"))
            }
        }
        // ... other error types
    }
}
```

### E.3 Output Size Contract

The Tool Executor provides `ctx.max_output_bytes` and `ctx.available_capacity_bytes`. WebFetch MUST:

1. Compute `effective_max_bytes = min(ctx.max_output_bytes, ctx.available_capacity_bytes)`
2. Ensure serialized JSON response ≤ `effective_max_bytes`
3. Set `ctx.allow_truncation = false` to prevent framework truncation marker

### E.4 Approval Summary

`approval_summary()` MUST return a redacted summary for user approval:

```
web_fetch https://example.com/path (http mode)
```

Query strings MUST be omitted to prevent credential leakage in approval prompts.

### E.5 Configuration Binding

WebFetch config (`tools.webfetch.*`) is loaded during tool registration:

1. `ForgeConfig::tools.webfetch` → `WebFetchConfig`
2. `WebFetchConfig` → `WebFetchTool::new(config)`
3. Tool registered in `ToolRegistry` if `enabled=true` and `tools.mode != disabled`

See §5.2 for precedence rules between global and tool-specific config.

