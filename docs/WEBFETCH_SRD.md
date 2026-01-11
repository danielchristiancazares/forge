# WebFetch Tool
## Software Requirements Document
**Version:** 2.2
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

### 0.4 GPT-5.2p compatibility review remediation (2026-01-10)
* **Status:** Addressed blocking issues and compatibility concerns from implementation review
* **Caching (E1):** **BREAKING** - Cache now stores canonical extracted document, not chunked response; re-chunking occurs per-request
* **Truncation semantics (D2):** `truncated` now means "content incomplete for any reason"; added `truncation_reason` enum with explicit values
* **Robots UA selection:** Declared deviation from RFC 9309 merge semantics; uses "most specific group wins" with explicit rationale
* **Appendix E:** Updated to match TOOL_EXECUTOR_SRD.md signatures; marked as illustrative, not normative
* **Robots 4xx (B3):** Added FR-WF-09c specifying allow-all behavior for non-404 4xx responses
* **Config fields:** Added `max_dns_attempts`, `max_cache_bytes`, `robots.user_agent_token` to configuration
* **DNS ordering (A3):** Made connection attempt order deterministic (sorted by IP bytes)
* **Browser isolation (C3):** Added FR-WF-BROWSER-ISO-01 requiring ephemeral browser profiles
* **Browser budgets (C4):** Added FR-WF-BROWSER-BUDGET-01 for subresource download limits
* **Browser methods (C2):** Added FR-WF-BROWSER-METHOD-01 restricting subrequests to GET/HEAD
* **URL validation (A1):** Added FR-WF-04b1/04b2 for raw-host numeric form detection
* **IDNA (A2):** Pinned to WHATWG URL Standard 'domain to ASCII' algorithm
* **Robots size (B5):** Added FR-WF-ROBOTS-SIZE-01 with 512 KiB limit
* **Robots parsing (B4):** Specified permissive line-level parsing per RFC 9309
* **Notes ordering:** Added FR-WF-NOTES-ORDER-01 with deterministic pipeline-stage ordering
* **Removed `max_chunks_reached`:** Removed undefined truncation reason from schema
* **Tests:** Added T-WF-ROB-11/12 for multi-group and 4xx robots behavior; T-WF-CCH-07 for cache re-chunking

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
* `truncated` (boolean) — true if returned content is incomplete for any reason (see FR-WF-TRUNC-01)
* `truncation_reason` (optional string, enum) — reason for truncation per FR-WF-TRUNC-REASON-01
* `notes` (array of strings, required, default `[]`) — stable tokens indicating conditions that occurred during fetch

**FR-WF-TRUNC-01 (Truncation semantics):** `truncated` MUST be `true` when the returned content is incomplete due to ANY of:
* Output byte budget enforcement (§3.5.4, FR-WF-OUT-01)
* Browser DOM snapshot taken before network idle (FR-WF-11f)
* Browser DOM exceeded size limit (FR-WF-11d)
* Response body exceeded download limit (reserved for future versions; current behavior returns `response_too_large` per FR-WF-10e, so this condition does not occur)

**FR-WF-TRUNC-REASON-01 (Truncation reason enum):** When `truncated=true`, `truncation_reason` MUST be one of the following values:
| Value | Condition |
|-------|-----------|
| `tool_output_limit` | Output byte budget enforcement truncated chunks |
| `browser_timeout_dom_partial` | Browser DOM extracted before network idle timeout |
| `browser_dom_truncated` | Browser DOM exceeded `max_rendered_dom_bytes` |

The `truncation_reason` field MUST be omitted when `truncated=false`.

**FR-WF-TRUNC-REASON-02 (Truncation reason precedence):** If multiple truncation conditions occur, `truncation_reason` MUST use the highest-precedence reason in this order:
1. `tool_output_limit`
2. `browser_dom_truncated`
3. `browser_timeout_dom_partial`

**FR-WF-RESP-URL-01 (URL field semantics):** `requested_url` MUST equal the original `args.url` string exactly as provided by the caller (no normalization). `final_url` MUST be the canonicalized URL (Appendix A) of the last fetched URL **with fragment removed**. If no redirects occurred, `final_url` is the canonicalized form of `requested_url`.

**FR-WF-RESP-METHOD-01 (Rendering method semantics):** `rendering_method` MUST reflect the method that produced the returned `chunks`. If browser fallback is used, `rendering_method` MUST be `"browser"` and `final_url` MUST reflect the browser navigation chain. If browser fallback was selected but unavailable, `rendering_method` MUST be `"http"` and `"browser_unavailable_used_http"` MUST be appended to `notes`.

**FR-WF-03c (Notes array):** The `notes` array replaces the singular `note` field. Defined tokens:
| Token | Condition |
|-------|-----------|
| `cache_hit` | Response served from cache |
| `robots_unavailable_fail_open` | robots.txt unavailable but `fail_open=true` |
| `browser_unavailable_used_http` | Browser fallback requested but unavailable |
| `browser_timeout_dom_partial` | Browser render timed out; partial DOM extracted |
| `browser_dom_truncated` | DOM exceeded `max_rendered_dom_bytes` |
| `browser_blocked_non_get` | Browser blocked non-GET/HEAD subrequests |
| `charset_fallback` | Unknown charset; fell back to UTF-8 with replacement |
| `cache_write_failed` | Cache write failed (fetch still succeeded) |
| `tool_output_limit` | Output truncated to fit byte budget |

**FR-WF-NOTES-ORDER-01 (Notes ordering):** The `notes` array MUST be ordered by pipeline stage to ensure deterministic output across implementations. The canonical order is:
1. `cache_hit` (cache layer)
2. `robots_unavailable_fail_open` (robots layer)
3. `browser_unavailable_used_http` (rendering selection)
4. `browser_timeout_dom_partial` (browser execution)
5. `browser_dom_truncated` (browser execution)
6. `browser_blocked_non_get` (browser execution)
7. `charset_fallback` (content decoding)
8. `cache_write_failed` (post-fetch)
9. `tool_output_limit` (output enforcement)

Tokens not present are omitted. This ordering ensures golden test stability across implementations.

**FR-WF-03d (Notes required):** The `notes` field MUST always be present in the response JSON. If no tokens apply, it MUST be an empty array `[]`.

**FR-WF-03a (Output size enforcement):** The **WebFetch tool's `execute()` method** MUST set `ctx.allow_truncation = false` before returning output, preventing the framework's generic truncation marker (`"... [output truncated]"`) from invalidating the JSON response. The tool MUST ensure serialized JSON size is `<= effective_max_bytes`, where `effective_max_bytes = min(ctx.max_output_bytes, ctx.available_capacity_bytes)` per `ToolCtx`.

**FR-WF-OUT-01 (Canonical output enforcement algorithm):** The tool MUST enforce `effective_max_bytes` using this deterministic algorithm:
1. Build all chunks per the chunking algorithm (§3.5.3).
2. Serialize the full response JSON. If within `effective_max_bytes`, return.
3. If over limit, drop chunks from the end one-by-one until payload fits OR only one chunk remains. Set `truncated=true` and `truncation_reason="tool_output_limit"`.
4. If still over limit with exactly one chunk, truncate that chunk's `text` at a UTF-8 boundary until payload fits.
5. After truncation, `token_count` MUST be recomputed for the final chunk text.

**FR-WF-OUT-02 (Minimal payload fallback):** If the payload still exceeds `effective_max_bytes` after truncating the final chunk's `text` to an empty string, the tool MUST return a structured JSON error with `code="internal"`, `message="tool_output_limit"`, and `retryable=false`. This prevents emitting invalid JSON.

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

**FR-WF-04c (Userinfo rejection precedence):** If the input URL contains a username or password component, the tool MUST return `invalid_url` and MUST NOT proceed to numeric-host detection, SSRF checks, or robots checks.

**FR-WF-04b (IP literal parsing):** Hostnames parsed as IP literals MUST be validated as IPs. Non-canonical numeric forms MUST be rejected with `invalid_host` error. Forbidden forms:
* Single-integer (dword) form: `2130706433`
* Hex form with `0x` prefix: `0x7f000001`
* Octal form with leading zeros: `0177.0.0.1`
* Mixed-base dotted forms: `0x7f.0.0.1`

**FR-WF-04b1 (Numeric host form detection):** Prior to WHATWG URL parsing, the implementation MUST extract the raw host substring from the input URL's authority component. The raw host substring MUST exclude any `userinfo@` prefix and any `:port` suffix, and spans from `://` to the next `/`, `?`, `#`, or end.

If the raw host starts with `[` and contains a matching `]`, treat the contents inside the brackets as the host literal for raw-host purposes (IPv6); numeric IPv4 form detection does NOT apply to bracketed IPv6 literals.

If the URL parser subsequently normalizes the host to an IPv4 address, but the raw host substring does NOT match the canonical dotted-decimal regex below, the tool MUST return `invalid_host` error with `details.host` set to the raw host substring. This prevents non-canonical numeric forms from being silently normalized.

Canonical dotted-decimal regex (no leading-zero octets except "0"):
```
^(?:0|[1-9][0-9]{0,2})(?:\.(?:0|[1-9][0-9]{0,2})){3}$
```

**FR-WF-04b2 (Error-code precedence):** For non-canonical numeric IPv4 forms, `invalid_host` MUST take precedence over `ssrf_blocked`, even if the normalized IP address would be blocked. The rationale: clearly identifying the input format issue helps callers fix their requests.

**Test vectors (all MUST be rejected with `invalid_host`):**
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

**FR-WF-05g (Additional blocked CIDRs):** `tools.webfetch.security.additional_blocked_cidrs` MUST be parsed as CIDR strings. Invalid entries MUST cause a configuration error at startup. These CIDRs are ALWAYS blocked regardless of `allow_insecure_overrides` and are additive to the built-in ranges.

#### 3.2.3 Port Policy

**FR-WF-05c (Allowed ports):** Only ports 80 and 443 are allowed by default.

**FR-WF-PORT-01 (Port allowlist semantics):** `allowed_ports` is the **complete allowlist** (override, not additive). Default is `[80, 443]`. If configured to a non-empty list, **only those ports** are permitted—the default ports are NOT implicitly included.
* Example: `allowed_ports = [8080]` means ONLY port 8080 is allowed; ports 80/443 are blocked
* Example: `allowed_ports = [80, 443, 8080]` allows all three
* Empty list (`[]`) means "use default `[80, 443]`"

**FR-WF-05d (Port allowlist config):** Additional ports are allowed by setting the complete list: `tools.webfetch.security.allowed_ports = [80, 443, 8080, 8443]`.

**FR-WF-05e (Port enforcement):** If a URL specifies a port not in the allowlist, the tool MUST return `port_blocked` error.

**FR-WF-05f (Implicit port resolution):** If the URL omits an explicit port, the tool MUST treat the port as `80` for `http` and `443` for `https` for all port allowlist checks, robots origin scoping, and cache key derivation.

#### 3.2.4 DNS Resolution and Rebinding Mitigation

**FR-WF-06 (DNS resolution):** Before any HTTP connection, DNS resolution MUST be performed and ALL resolved IPs MUST pass SSRF checks.

**FR-WF-06a (TOCTOU mitigation - HTTP mode):** The implementation MUST use a DNS resolver/connector that pins the resolved IP set during validation. The TCP connection MUST only be made to IPs that passed SSRF validation. This prevents DNS rebinding attacks.

**FR-WF-DNS-01 (Deterministic connection strategy):** The resolver MUST return all resolved IPs. Connection behavior:
1. **Sort IPs deterministically:** IPv6 addresses sorted lexicographically by 16-byte value (ascending), then IPv4 addresses sorted lexicographically by 4-byte value (ascending). This ensures identical attempt order regardless of resolver implementation or platform.
2. Attempt connections in sorted order, trying at most `max_dns_attempts` addresses (configurable, default: 2) before failing.
3. Re-resolution during a fetch is **forbidden**—use only the initially resolved set.
4. If all attempted addresses fail, return `network` error with details.

**FR-WF-DNS-02 (max_dns_attempts configuration):** `max_dns_attempts` MUST be a configuration value under `tools.webfetch.security.max_dns_attempts` (default: 2, range: 1-10). This caps connection attempts and prevents excessive latency when multiple addresses are returned.

**Example:** Given resolver returns `[A:10.0.0.2, AAAA:2001:db8::2, AAAA:2001:db8::1, A:10.0.0.1]`, the tool attempts in order: `2001:db8::1`, `2001:db8::2` (IPv6 sorted), then `10.0.0.1`, `10.0.0.2` (IPv4 sorted). With `max_dns_attempts=2`, only the first two are attempted.

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

**FR-WF-08b (Parsing algorithm):** robots.txt MUST be parsed with the following semantics:
1. Select the most specific user-agent group matching the configured UA token (see FR-WF-ROBOTS-UA-TOKEN-01)
2. If no matching group, use the `*` (wildcard) group
3. If no `*` group exists, treat as allow-all

**FR-WF-ROBOTS-UA-01 (User-agent specificity - DEVIATION FROM RFC 9309):** This implementation uses "most specific group wins" semantics rather than RFC 9309's group-merge semantics. Rationale: simpler implementation, predictable behavior, and aligns with common crawler practice.

A group matches if any `User-agent` line contains the UA token as a case-insensitive substring. For a group with multiple `User-agent` lines, its specificity is the **maximum** length among the matching `User-agent` values within that group. Among matching groups, choose the group with the **longest** matching `User-agent` value; tie-break by file order (first wins). **Groups are NOT merged.**
* Example: UA token `forge`, groups `User-agent: forge-webfetch` and `User-agent: forge` → select `forge-webfetch` (longer match)
* Example: Multiple groups match `forge` → use first matching group only, not union of rules

**FR-WF-ROBOTS-UA-TOKEN-01 (User-agent token):** Robots user-agent matching MUST use a dedicated token configured at `tools.webfetch.robots.user_agent_token`. If absent, derive it by:
1. Taking the configured HTTP `user_agent` string
2. Extracting the product name before the first `/` character (e.g., `forge-webfetch` from `forge-webfetch/1.0`)
3. Stripping any characters not in `[A-Za-z0-9_-]`
4. If the derived token is empty, default to `forge-webfetch`

This token is used solely for robots.txt group matching, not for HTTP headers.

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

**FR-WF-08e (User-agent matching):** User-agent group selection MUST be case-insensitive substring matching against `tools.webfetch.robots.user_agent_token` only (see FR-WF-ROBOTS-UA-TOKEN-01).

**FR-WF-ROBOTS-FETCH-01 (Redirect handling):** robots.txt retrieval MUST follow redirects up to `max_redirects`, applying the same SSRF validation and port policy per hop as document fetching. A redirect from `http://` to `https://` for robots.txt is common and MUST be followed.

**FR-WF-ROBOTS-REDIR-02 (Cross-origin redirect policy):** robots.txt redirects MUST remain within the original origin `(scheme, host, port)`. If a redirect targets a different origin, treat it as robots.txt unavailable:
* If `tools.webfetch.robots.fail_open = true`: allow-all and append `"robots_unavailable_fail_open"` to `notes`
* If `tools.webfetch.robots.fail_open = false`: return `robots_unavailable` error with details `{ "origin": "...", "error": "robots_cross_origin_redirect" }`

#### 3.3.3 Fetch Failure Behavior

**FR-WF-09 (HTTP 404):** If robots.txt returns HTTP 404, treat as allow-all.

**FR-WF-09a (Network/timeout failure):** If robots.txt fetch fails (DNS error, timeout, connection refused, HTTP 5xx):
* If `tools.webfetch.robots.fail_open = true` (default: false): treat as allow-all, append `"robots_unavailable_fail_open"` to `notes`
* If `tools.webfetch.robots.fail_open = false`: return `robots_unavailable` error

**FR-WF-09b (Malformed robots.txt):** Robots parsing MUST be permissive at the line level per RFC 9309:
1. Unknown fields and invalid lines MUST be ignored (not cause parse failure)
2. Valid directives in valid groups MUST still apply even if other lines are invalid
3. The entire file MUST be treated as allow-all ONLY if:
   * It cannot be decoded as UTF-8 (after optional BOM handling), OR
   * No valid group/directive pairs can be parsed from the file
4. Empty files are treated as allow-all

**FR-WF-09c (HTTP 4xx behavior):** If robots.txt retrieval returns any HTTP 4xx status code (including 401, 403, 429, etc.), the tool MUST treat it as allow-all. This aligns with RFC 9309's "Unavailable" status handling. The tool MUST NOT return `robots_unavailable` for 4xx responses.

**FR-WF-ROBOTS-SIZE-01 (Size limit):** robots.txt retrieval MUST enforce a maximum size limit of `tools.webfetch.robots.max_robots_bytes` (default: 524288, i.e., 512 KiB). If the response body exceeds this limit:
1. Stop reading further bytes
2. Parse only the downloaded prefix
3. Log a warning: `"robots.txt truncated at {max_robots_bytes} bytes"`
4. Do NOT append a note token (this is a silent limit, not a fetch condition)

#### 3.3.4 Caching

**FR-WF-08f (Cache TTL):** robots.txt cache entries MUST have a TTL (default: 24 hours, configurable via `robots_cache_ttl_hours`). Entries MUST be revalidated after TTL expiry.

**FR-WF-08g (Cache eviction):** When `robots_cache_entries` limit is reached, evict entries using LRU (Least Recently Used).

**FR-WF-08h (no_cache interaction):** `no_cache=true` in the request MUST NOT bypass robots.txt enforcement. robots.txt caching operates independently.

**FR-WF-ROBOTS-CACHE-03 (Disable caching):** If `robots_cache_entries = 0`, robots.txt caching MUST be disabled (no cache reads or writes).

**FR-WF-ROBOTS-CACHE-01 (Cache scope):** robots.txt cache MUST be process-local in-memory only (no on-disk persistence). Cache state is cleared on process restart.

**FR-WF-ROBOTS-CACHE-02 (Allow-all caching):** Allow-all outcomes (HTTP 404/4xx, empty file, malformed file treated as allow-all) MUST be cached with the same TTL and LRU semantics as normal robots.txt decisions.

**FR-WF-ROBOTS-CACHE-04 (Fail-open caching):** Allow-all outcomes produced via `fail_open=true` MUST NOT be cached. Each fetch attempt must re-evaluate robots.txt availability to avoid persisting a transient failure.

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

**FR-WF-10e (Response size limit):** The tool MUST enforce `max_download_bytes` (default: 5 MiB) on **decompressed** bytes. If exceeded during download, abort and return `response_too_large` error. Implementations MAY also cap compressed bytes to the same limit; if they do, the same `response_too_large` error MUST be returned.

#### 3.4.2 Content-Type Handling

**FR-WF-10f (Supported content types):** Only the following content types are supported:
* `text/html`, `application/xhtml+xml` → HTML extraction pipeline
* `text/plain` → pass-through with minimal normalization

**FR-WF-CT-01 (Content-Type parsing):** Content-Type parsing MUST:
1. Use only the media type (`type/subtype`) for content-type selection
2. Ignore parameters for supported/unsupported checks (except `charset`, see FR-WF-10i)
3. Compare media types case-insensitively after trimming whitespace

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

**FR-WF-CHARSET-02 (Charset precedence):** Charset selection precedence MUST be:
1. `Content-Type` header charset (if present and parseable)
2. HTML `<meta charset>` / `<meta http-equiv="Content-Type">` (only when header charset is absent)
3. UTF-8 fallback with replacement (and append `"charset_fallback"` when unknown/unsupported)

**FR-WF-CHARSET-01 (Charset normalization):** Charset names MUST be matched case-insensitively. Supported charsets: `UTF-8`, `ISO-8859-1` (Latin-1), `Windows-1252`. Unknown charsets MUST fall back to UTF-8 with replacement and append `"charset_fallback"` to `notes`.

#### 3.4.3 Browser Mode

**FR-WF-11 (Browser implementation):** Browser mode MUST be implemented by:
1. Spawning the Chromium binary at `chromium_path` (or from PATH if empty)
2. Using headless mode with DevTools Protocol (CDP) via a CDP client library
3. Minimum Chromium version: 100 (for stable CDP API)

**FR-WF-BROWSER-ISO-01 (Browser isolation):** Browser mode MUST launch Chromium with a fresh temporary user-data-dir per tool invocation. Requirements:
1. Create a unique temporary directory for the browser profile
2. Delete the directory after browser session completes (success or failure)
3. Persistent cookies, localStorage, and disk cache MUST NOT survive across invocations
4. Use `--user-data-dir={temp_dir}` flag to enforce isolation

This prevents cross-invocation data leakage, ensures deterministic behavior, and protects user privacy.

**FR-WF-11a (Browser unavailable):** If Chromium is unavailable and browser mode is required (`force_browser=true`), return `browser_unavailable` error.

**FR-WF-11b (Browser SSRF enforcement - BLOCKING):** Browser mode MUST intercept **all** network requests (document, script, XHR/fetch, iframe, websocket initiation, subresources) and MUST apply SSRF validation (DNS + IP range checks) per-request.

**FR-WF-ROBOTS-SCOPE-01 (Robots scope in browser mode):** robots.txt checks apply only to the main document navigation chain (top frame) and its redirects. Subresource requests (scripts, images, XHR, fetch, iframes) MUST NOT be blocked by robots.txt, but MUST still pass SSRF validation per FR-WF-11b.

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

**FR-WF-BSSR-TLS-01 (Pinned TLS verification):** When fetching `https://` resources via either architecture:
1. The connection MUST use SNI and `Host` corresponding to the original request host (not the pinned IP)
2. Certificate validation MUST be performed against the original host name
3. If certificate validation fails, return `network` error with details `{ "error": "tls_validation_failed" }`

Pinning IPs MUST NOT disable or weaken TLS hostname verification.

**FR-WF-BROWSER-METHOD-01 (Subrequest method restrictions):** In browser mode, only `GET` and `HEAD` subrequests are permitted by default. For any other HTTP method (POST, PUT, DELETE, etc.):
1. The request MUST be failed via `Fetch.failRequest` with `BlockedByClient`
2. Increment an internal counter of blocked non-GET requests
3. If any such failures occurred during the page load, append `"browser_blocked_non_get"` to `notes`

This prevents exfiltration via POST requests and ensures deterministic behavior. Future versions MAY add configurable POST support with explicit body size limits.

**FR-WF-BROWSER-BUDGET-01 (Subresource download budget):** In browser mode, each subresource fetch MUST enforce:
1. Per-resource limit: `max_download_bytes` (same value as HTTP mode, default 5 MiB)
2. Total page budget: `max_total_subresource_bytes` (configurable, default 20 MiB)

When a limit is exceeded:
* Per-resource: Fail that specific request; page loading continues
* Total budget: Fail all subsequent subresource requests; proceed to DOM extraction
* In either case, log a warning but do NOT append a note token (internal budget, not user-visible condition)

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

**FR-WF-RENDER-JS-01 (JS-heavy domain matching):** Each configured entry MUST be normalized using Appendix A host normalization (lowercase + IDNA to ASCII). Matching rules:
* If the entry starts with `.`, it matches the exact host suffix (subdomains), e.g., `.example.com` matches `a.example.com` but not `example.com`
* Otherwise, it matches the host exactly (no implicit subdomains)

**FR-WF-12c (SPA fallback heuristic):** After HTTP extraction, trigger browser fallback when ALL of:
1. Extracted markdown length `< min_extracted_chars` (default: 400)
2. The **raw decoded HTML** (before boilerplate removal/extraction) contains SPA indicators: `<script type="module">`, `id="__next"`, `id="app"`, `id="root"`, `window.__NUXT__`, `window.__INITIAL_STATE__` (case-insensitive substring match)
3. HTTP status was 200

**FR-WF-12d (Browser fallback unavailable):** If browser fallback is selected but browser is unavailable:
* Return HTTP result with `"browser_unavailable_used_http"` appended to `notes`

**FR-WF-12e (Fallback disabled):** SPA fallback MAY be disabled via `tools.webfetch.rendering.spa_fallback_enabled = false`.

**FR-WF-12f (No fallback on HTTP errors):** Browser fallback MUST NOT be attempted after HTTP-mode errors (timeout, network, 4xx/5xx, unsupported_content_type, response_too_large, extraction_failed). In these cases, the HTTP-mode error MUST be returned unless `force_browser=true` or the domain is configured in `js_heavy_domains` (in which case browser mode is selected before HTTP).

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
1. Match the list marker regex: `^\s{0,3}(?:[-+*]|\d+[.)])\s+`
2. Include continuation lines: lines that do NOT match the list marker regex but are indented by at least two spaces or a tab, immediately following a list item line without an intervening blank line
3. End at: a blank line followed by non-list content, OR a heading, OR end of document

List items at different nesting levels are part of the SAME list block if contiguous. The entire nested structure forms one atomic block for chunking purposes.

**FR-WF-CHK-LIST-02 (Oversized list splitting):** If a list block exceeds `max_chunk_tokens`, the tool MUST first attempt to split the block at list-item boundaries (preserving complete items). Only if a single list item still exceeds `max_chunk_tokens` may sentence/whitespace splitting be applied within that item.

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
1. If the block is a list block, apply FR-WF-CHK-LIST-02
2. Otherwise split at sentence boundaries (`.!?` followed by space or EOL) if possible
3. Else split at whitespace boundaries
4. Else split at UTF-8 character boundary (never mid-codepoint)
5. Each split piece becomes its own chunk

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

**FR-WF-15e (Chunk limiting):** Output size enforcement MUST follow FR-WF-OUT-01 (§3.1.3). This section contains no additional normative rules beyond FR-WF-OUT-01. Summary:
1. Build full response, then apply FR-WF-OUT-01/FR-WF-OUT-02 trimming rules
2. If content was limited: set `truncated=true` and `truncation_reason="tool_output_limit"`
3. If the first chunk alone exceeds budget, return it truncated (not zero chunks)

**FR-WF-15f (Chunk ordering):** Chunks MUST be returned in document order (first chunk = beginning of content).

### 3.6 Caching

**FR-WF-CCH-ENABLE-01 (Cache enablement):** Caching is enabled only when `cache_dir` resolves to a non-empty, creatable directory AND `max_cache_entries > 0`. If caching is disabled, the tool MUST skip both cache reads and writes, MUST NOT emit `cache_hit` or `cache_write_failed`, and `no_cache` has no effect.

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
* **Chunking and output fitting are ALWAYS recomputed on cache hit** using current request parameters (see FR-WF-CCH-CONTENT-01)

**FR-WF-CCH-KEY-03 (Fallback caching):** If HTTP-first rendering falls back to browser mode, only the browser-rendered result MUST be cached. The cache key MUST use the browser method and the browser `final_url`. The intermediate HTTP extraction MUST NOT be cached.

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
  "version": 2,
  "canonical_url": "https://example.com/page",
  "rendering_method": "http",
  "fetched_at": "2026-01-10T12:00:00Z",
  "expires_at": "2026-01-17T12:00:00Z",
  "last_accessed_at": "2026-01-10T14:30:00Z",
  "extracted": {
    "markdown": "# Page Title\n\nContent here...",
    "title": "Page Title",
    "language": "en"
  }
}
```

**FR-WF-CCH-VER-01 (Entry format versioning):** The `version` field enables forward compatibility:
1. Current version: `2` (changed from v1 which stored chunked response)
2. On read, if `version > SUPPORTED_VERSION`, treat as cache miss (don't attempt to parse)
3. On read, if `version < CURRENT_VERSION`, treat as cache miss and re-fetch (no migration)
4. Version increments when `extracted` schema changes incompatibly

**FR-WF-CCH-CONTENT-01 (Extracted content storage):** The `extracted` field stores the **canonical extracted document**, NOT the chunked response. This includes:
* `markdown` (string): The full extracted markdown content before chunking
* `title` (optional string): Extracted page title
* `language` (optional string): Extracted language tag

On cache hit, the tool MUST:
1. Read the cached `extracted` content
2. Re-apply chunking (FR-WF-15) using the **current request's** `max_chunk_tokens`
3. Re-apply output fitting (FR-WF-OUT-01) using the **current** `effective_max_bytes`
4. Build the response with `fetched_at` from cache metadata

This design ensures that `max_chunk_tokens` variations between requests produce correct output without cache key proliferation.

#### 3.6.3 TTL and Eviction

**FR-WF-16d (TTL):** Cache entries MUST have a TTL. Default: 7 days, configurable via `cache_ttl_days`. Entries with `expires_at < now` MUST be treated as cache miss.

**FR-WF-16e (Eviction policy):** The cache MUST enforce `max_cache_entries` (default: 10000) with LRU eviction. On write, if limit is reached, evict least-recently-used entries before writing.

**FR-WF-CCH-LRU-01 (LRU tracking):** "Recently used" is determined by the `last_accessed_at` timestamp in cache metadata:
1. On cache **read** (hit): update `last_accessed_at` to current time using atomic rewrite
2. On cache **write**: set `last_accessed_at` to current time
3. Eviction selects entries with the oldest `last_accessed_at` timestamp
4. Ties MUST be broken by lexicographic ordering of `cache_key` (ensures determinism)

The `last_accessed_at` field MUST be stored in RFC3339 UTC format in the JSON metadata (as shown in FR-WF-16c). This is the REQUIRED approach for deterministic behavior and portability across filesystems.

**FR-WF-16f (Size limit):** Optionally enforce `max_cache_bytes` (default: 1 GiB). If set, evict LRU entries until under budget.

**FR-WF-CCH-SIZE-01 (Oversized entry):** If a single cache entry's serialized size exceeds `max_cache_bytes`, the tool MUST skip the cache write, log `"cache_write_failed: entry exceeds max_cache_bytes"`, and append `"cache_write_failed"` to `notes`.

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
2. MUST still write fresh result to cache (overwriting any existing entry) when caching is enabled (see FR-WF-CCH-ENABLE-01)
3. MUST NOT bypass robots.txt enforcement (robots cache is independent)

**FR-WF-CCH-NOCACHE-01 (no_cache write behavior):** `no_cache=true` means "don't read from cache" but DOES write:
* The fresh fetch result overwrites any existing cache entry for the same key
* This allows `no_cache` to be used for cache refresh/warming
* To completely avoid cache interaction, implementers must disable caching at config level (FR-WF-CCH-ENABLE-01)

**FR-WF-17a (Cache hit response):** On cache hit:
* `fetched_at` = original fetch time from cache metadata (not current time)
* Append `"cache_hit"` to `notes`
* Re-chunk and re-apply output fitting per FR-WF-CCH-CONTENT-01

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

**FR-WF-ERR-BADARGS-01 (Tool-level validation errors):** Any additional request validation failures inside `web_fetch` (e.g., out-of-range `max_chunk_tokens`, unknown `block_resources`, invalid `allowed_ports`) MUST return a structured JSON error envelope with `code="bad_args"` via `ToolError::ExecutionFailed`. `ToolError::BadArgs` MUST be reserved for schema validation only.

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
| `http_4xx` | HTTP 4xx client error | No (except 408/429: Yes) | `status`, `status_text` |
| `http_5xx` | HTTP 5xx server error | Yes | `status`, `status_text` |
| `browser_unavailable` | Chromium not found/runnable | No | `chromium_path`, `error` |
| `browser_crashed` | Browser process crashed | Yes | `error` |
| `extraction_failed` | HTML extraction failed | No | `error` |
| `cache_read_failed` | Cache read error | Yes | `path`, `error` |
| `internal` | Unexpected internal error | Yes (except `tool_output_limit`: No) | `error` |

**FR-WF-ERR-HTTP-01 (Retryable 4xx):** For HTTP status 408 or 429, `retryable` MUST be `true` while still using `code="http_4xx"`. All other 4xx statuses use `retryable=false`.

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
    pub max_cache_bytes: Option<u64>,  // Added: max total cache size (default: 1 GiB)
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

#[derive(Debug, Deserialize, Default)]
pub struct WebFetchSecurityConfig {
    pub block_private_ips: Option<bool>,
    pub block_loopback: Option<bool>,
    pub block_link_local: Option<bool>,
    pub block_reserved: Option<bool>,
    pub allowed_ports: Option<Vec<u16>>,
    pub additional_blocked_cidrs: Option<Vec<String>>,
    pub allow_insecure_overrides: Option<bool>,
    pub max_dns_attempts: Option<u32>,  // Added: max DNS connection attempts (default: 2)
}

#[derive(Debug, Deserialize, Default)]
pub struct WebFetchBrowserConfig {
    pub enabled: Option<bool>,
    pub chromium_path: Option<String>,
    pub network_idle_ms: Option<u32>,
    pub max_rendered_dom_bytes: Option<u64>,
    pub max_total_subresource_bytes: Option<u64>,  // Added: total subresource budget (default: 20 MiB)
    pub block_resources: Option<Vec<String>>,
}

#[derive(Debug, Deserialize, Default)]
pub struct WebFetchHttpConfig {
    pub use_system_proxy: Option<bool>,
}

#[derive(Debug, Deserialize, Default)]
pub struct WebFetchRenderingConfig {
    pub js_heavy_domains: Option<Vec<String>>,
    pub spa_fallback_enabled: Option<bool>,
    pub min_extracted_chars: Option<u32>,
}

#[derive(Debug, Deserialize, Default)]
pub struct WebFetchRobotsConfig {
    pub fail_open: Option<bool>,
    pub user_agent_token: Option<String>,  // Added: dedicated robots UA token
    pub max_robots_bytes: Option<u64>,     // Added: max robots.txt size (default: 512 KiB)
}
```

**FR-WF-CFG-02 (Tool settings):** Extend `tools::ToolSettings` to carry `WebFetchSettings` for registration.

**FR-WF-CFG-03 (Env var expansion):** Apply `config::expand_env_vars()` to `cache_dir` and `chromium_path`.

**FR-WF-CFG-03a (Expanded path handling):**
* If the expanded `cache_dir` is empty or whitespace-only, caching is disabled (FR-WF-CCH-ENABLE-01).
* If `cache_dir` is relative, resolve it against the process working directory.
* If the expanded `chromium_path` is empty, Chromium MUST be searched via PATH (existing behavior).
* If `chromium_path` is relative, resolve it against the process working directory.

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
cache_dir = "${TEMP}/forge-webfetch" # Cache directory (env vars expanded, empty disables caching)
cache_ttl_days = 7                   # Cache entry TTL (default: 7)
max_cache_entries = 10000            # Max cached entries (default: 10000, 0 disables caching)
max_cache_bytes = 1073741824         # Max total cache size in bytes (default: 1 GiB)
max_download_bytes = 5242880         # Max response size in bytes (default: 5 MiB)
robots_cache_entries = 1024          # Max robots.txt cache entries (default: 1024)
robots_cache_ttl_hours = 24          # robots.txt cache TTL (default: 24)
allow_auto_execution = false         # Skip approval prompt (default: false, use with caution)

[tools.webfetch.http]
use_system_proxy = false             # Use HTTP_PROXY/HTTPS_PROXY (default: false)

[tools.webfetch.robots]
fail_open = false                    # Allow fetch if robots.txt unavailable (default: false)
user_agent_token = "forge-webfetch"  # Dedicated token for robots.txt matching
max_robots_bytes = 524288            # Max robots.txt size (default: 512 KiB)

[tools.webfetch.browser]
enabled = true                       # Enable browser mode/fallback (default: true)
chromium_path = ""                   # Path to Chromium binary (empty = search PATH)
max_total_subresource_bytes = 20971520  # Total page subresource budget (default: 20 MiB)
network_idle_ms = 20000              # Max wait for network idle (default: 20000)
max_rendered_dom_bytes = 5242880     # Max DOM size (default: 5 MiB)
block_resources = ["image", "font", "media"]  # CDP ResourceTypes to block

[tools.webfetch.security]
block_private_ips = true             # Block RFC 1918 ranges (default: true)
block_loopback = true                # Block 127.0.0.0/8, ::1 (default: true)
block_link_local = true              # Block 169.254.0.0/16, fe80::/10 (default: true)
block_reserved = true                # Block other reserved ranges (default: true)
allowed_ports = [80, 443]            # Allowed destination ports (default: [80, 443])
additional_blocked_cidrs = []        # Additional CIDRs to always block (default: [])
max_dns_attempts = 2                 # Max DNS connection attempts (default: 2, range: 1-10)
allow_insecure_overrides = false     # Required to disable SSRF protections (default: false)

[tools.webfetch.rendering]
js_heavy_domains = []                # Domains that skip HTTP mode (default: [])
spa_fallback_enabled = true          # Enable SPA detection fallback (default: true)
min_extracted_chars = 400            # Threshold for SPA fallback (default: 400)
```

### 5.4 Configuration Validation

| Field | Type | Default | Range | Notes |
|-------|------|---------|-------|-------|
| `timeout_seconds` | u32 | 20 | 1-300 | Clamped to range |
| `max_redirects` | u32 | 5 | 0-20 | Clamped to range |
| `default_max_chunk_tokens` | u32 | 600 | 128-2048 | Clamped to range |
| `cache_ttl_days` | u32 | 7 | 1-365 | Clamped to range |
| `max_cache_entries` | u32 | 10000 | 0-1000000 | Clamped to range; 0 disables caching |
| `max_cache_bytes` | u64 | 1073741824 | 1048576-1099511627776 | 1 MiB to 1 TiB, clamped |
| `max_download_bytes` | u64 | 5242880 | 1024-104857600 | 1 KiB to 100 MiB, clamped |
| `robots_cache_entries` | u32 | 1024 | 0-100000 | Clamped to range; 0 disables robots cache |
| `robots_cache_ttl_hours` | u32 | 24 | 1-720 | Clamped to range |
| `max_dns_attempts` | u32 | 2 | 1-10 | Clamped to range |
| `network_idle_ms` | u32 | 20000 | 1000-120000 | Clamped to range |
| `max_rendered_dom_bytes` | u64 | 5242880 | 1024-104857600 | 1 KiB to 100 MiB, clamped |
| `max_total_subresource_bytes` | u64 | 20971520 | 1048576-524288000 | 1 MiB to 500 MiB, clamped |
| `min_extracted_chars` | u32 | 400 | 0-10000 | Clamped to range |
| `allowed_ports` | Vec<u16> | [80, 443] | Valid port numbers | Empty = default |
| `cache_dir` | String | `${TEMP}/forge-webfetch` | Non-empty path | Empty disables caching |

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
| T-WF-SSRF-12 | DNS ordering: IPv6 sorted before IPv4, lexicographic by bytes | FR-WF-DNS-01 |
| T-WF-SSRF-13 | max_dns_attempts=2: only first two IPs attempted | FR-WF-DNS-02 |
| T-WF-SSRF-14 | Non-canonical numeric IP detection on raw host substring | FR-WF-04b1 |
| T-WF-SSRF-15 | Reject URLs with userinfo components | FR-WF-04c, FR-WF-URL-USERINFO-01 |
| T-WF-SSRF-16 | Reject dotted-decimal IPv4 with leading-zero octets | FR-WF-04b1 |

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
| T-WF-ROB-11 | Multiple matching UA groups: use first matching (not merge) | FR-WF-ROBOTS-UA-01 |
| T-WF-ROB-12 | robots.txt returns 403: treat as allow-all | FR-WF-09c |
| T-WF-ROB-13 | robots.txt exceeds size limit: parse prefix only | FR-WF-ROBOTS-SIZE-01 |
| T-WF-ROB-14 | UA token derived from user_agent when user_agent_token absent | FR-WF-ROBOTS-UA-TOKEN-01 |
| T-WF-ROB-15 | Group specificity uses longest matching UA line within group | FR-WF-ROBOTS-UA-01 |
| T-WF-ROB-16 | robots.txt cross-origin redirect treated as unavailable | FR-WF-ROBOTS-REDIR-02 |
| T-WF-ROB-17 | fail_open outcomes are not cached | FR-WF-ROBOTS-CACHE-04 |

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
| T-WF-CHK-06 | First chunk exceeds budget: truncated chunk returned (not zero) | FR-WF-15e, FR-WF-OUT-01 |
| T-WF-CHK-07 | Oversized list block splits at list item boundaries | FR-WF-CHK-LIST-02 |
| T-WF-OUT-01 | Notes ordered by pipeline stage | FR-WF-NOTES-ORDER-01 |
| T-WF-OUT-02 | UTF-8 truncation uses char boundary and recomputes token_count | FR-WF-OUT-01 |
| T-WF-OUT-03 | Truncation reason precedence when multiple causes apply | FR-WF-TRUNC-REASON-02 |
| T-WF-OUT-04 | notes field always present (empty array when none) | FR-WF-03d |

### 6.3a Unit Tests - Rendering Selection

| Test ID | Description | Requirement |
|---------|-------------|-------------|
| T-WF-REN-01 | js_heavy_domains exact vs suffix matching | FR-WF-RENDER-JS-01 |

### 6.4 Unit Tests - Caching

| Test ID | Description | Requirement |
|---------|-------------|-------------|
| T-WF-CCH-01 | Cache hit returns prior content with correct fetched_at | FR-WF-17a |
| T-WF-CCH-02 | no_cache=true bypasses read, still writes | FR-WF-17 |
| T-WF-CCH-03 | Expired entries treated as miss | FR-WF-16d |
| T-WF-CCH-04 | HTTP vs browser renders have separate cache keys | FR-WF-16 |
| T-WF-CCH-05 | Cache path uses hash, no URL components | FR-WF-16b |
| T-WF-CCH-06 | LRU eviction when limit reached | FR-WF-16e |
| T-WF-CCH-07 | Same URL with different max_chunk_tokens: re-chunking produces different output | FR-WF-CCH-CONTENT-01 |
| T-WF-CCH-08 | Cache hit updates last_accessed_at | FR-WF-CCH-LRU-01 |
| T-WF-CCH-09 | Cache version mismatch triggers re-fetch | FR-WF-CCH-VER-01 |
| T-WF-CCH-10 | Browser fallback caches browser result only | FR-WF-CCH-KEY-03 |
| T-WF-CCH-11 | Oversized cache entry skips write with note | FR-WF-CCH-SIZE-01 |

### 6.5 Unit Tests - Errors

| Test ID | Description | Requirement |
|---------|-------------|-------------|
| T-WF-ERR-01 | Error response is valid JSON with code/message/retryable | FR-WF-18 |
| T-WF-ERR-02 | All error codes in registry are produced by code paths | FR-WF-18 registry |
| T-WF-ERR-03 | Query strings redacted in logs | FR-WF-19b |
| T-WF-ERR-04 | HTTP 429/408 set retryable=true with code http_4xx | FR-WF-ERR-HTTP-01 |

### 6.6 Integration Tests

| Test ID | Description | Requirement |
|---------|-------------|-------------|
| IT-WF-HTTP-01 | Fetch and extract Markdown via HTTP (wiremock) | FR-WF-10, FR-WF-13 |
| IT-WF-HTTP-02 | Follow redirects, validate each hop | FR-WF-07 |
| IT-WF-HTTP-03 | Unsupported content-type returns error | FR-WF-10g |
| IT-WF-HTTP-04 | Response size limit enforced | FR-WF-10e |
| IT-WF-HTTP-05 | Charset detection (ISO-8859-1 → UTF-8) | FR-WF-10i |
| IT-WF-HTTP-06 | Header charset takes precedence over HTML meta | FR-WF-CHARSET-02 |
| IT-WF-HTTP-07 | Content-Type parameters ignored for type selection | FR-WF-CT-01 |

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
| IT-WF-BR-08 | Browser isolation: fresh profile per invocation | FR-WF-BROWSER-ISO-01 |
| IT-WF-BR-09 | POST subrequest blocked with note token | FR-WF-BROWSER-METHOD-01 |
| IT-WF-BR-10 | Subresource budget exceeded: subsequent requests failed | FR-WF-BROWSER-BUDGET-01 |

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

**FR-WF-NORM-07 (IDNA/Punycode):** Host normalization MUST use the WHATWG URL Standard "domain to ASCII" algorithm (UTS#46 processing) as implemented by the chosen WHATWG-compliant URL parser (e.g., Rust `url` crate). This ensures consistent handling of:
* Unicode normalization (NFC)
* Mapping rules (IDNA2008 compatibility mode)
* Error handling (transitional processing disabled)

Canonical URLs MUST use the ASCII form.
* Example: `https://münich.example/` → `https://xn--mnich-kva.example/`

**IDNA Test Vectors (normative):**
| Input Host | Expected ASCII Host | Notes |
|------------|---------------------|-------|
| `münchen.example` | `xn--mnchen-3ya.example` | German umlaut |
| `日本語.jp` | `xn--wgv71a119e.jp` | Japanese |
| `例え.jp` | `xn--r8jz45g.jp` | Japanese |
| `münchen.example` | `xn--mnchen-3ya.example` | Compatibility character |
| `EXAMPLE.COM` | `example.com` | Case folding |

**FR-WF-URL-USERINFO-01 (Userinfo rejection):** URLs containing a username or password component (e.g., `https://user:pass@example.com/`) MUST be rejected with `invalid_url` error. This prevents credential leakage into logs, caches, and approval summaries. The userinfo rejection check MUST occur before numeric-host detection (FR-WF-04b1) and before SSRF/robots validation.

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

This appendix documents how WebFetch integrates with Forge's Tool Executor framework. **This appendix is ILLUSTRATIVE, not normative. For authoritative trait definitions, refer to `docs/TOOL_EXECUTOR_SRD.md`.**

### E.1 ToolExecutor Trait Implementation

WebFetch MUST implement the `ToolExecutor` trait per TOOL_EXECUTOR_SRD.md FR-REG-01:

```rust
impl ToolExecutor for WebFetchTool {
    fn name(&self) -> &'static str { "web_fetch" }

    fn description(&self) -> &'static str {
        "Fetch a URL and extract its content as markdown"
    }

    fn is_side_effecting(&self) -> bool { true }  // Network egress

    // Note: requires_approval() takes no arguments per TOOL_EXECUTOR_SRD.md
    // Policy-based approval is handled by the framework, not the tool
    fn requires_approval(&self) -> bool {
        !self.config.allow_auto_execution
    }

    fn timeout(&self) -> Option<Duration> {
        // Must exceed internal timeouts (FR-WF-TIMEOUT-01)
        Some(Duration::from_secs(self.timeout_seconds + self.network_idle_ms/1000 + 7))
    }

    // Note: ctx is &mut per TOOL_EXECUTOR_SRD.md FR-REG-01
    fn execute<'a>(&'a self, args: Value, ctx: &'a mut ToolCtx) -> ToolFut<'a> {
        Box::pin(async move {
            // Set allow_truncation=false before returning (FR-WF-03a)
            ctx.allow_truncation = false;
            // ... implementation
            Ok(json_response)
        })
    }
}
```

**Important signature differences from earlier drafts:**
* `requires_approval()` takes no arguments (policy is handled by framework via `Policy` struct)
* `execute()` takes `ctx: &'a mut ToolCtx` (mutable reference), not by value
* `timeout()` returns `Option<Duration>`, not `Duration`

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
