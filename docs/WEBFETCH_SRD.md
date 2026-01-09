# WebFetch Tool
## Software Requirements Document
**Version:** 1.0  
**Date:** 2026-01-08  
**Status:** Draft  
**Baseline code reference:** `forge-source.zip`

---

## 0. Change Log
### 0.1 Initial draft
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
**FR-WF-01:** Tool name MUST be `WebFetch` with aliases `webfetch` and `web_fetch`.

**FR-WF-02:** Request schema MUST include:
* `url` (string, required)
* `max_chunk_tokens` (integer, optional)
* `no_cache` (boolean, optional, default false)
* `force_browser` (boolean, optional, default false)
* `additionalProperties` MUST be false

**FR-WF-03:** Response payload MUST include:
* `url` (string)
* `fetched_at` (ISO-8601 timestamp)
* `title` (optional string)
* `language` (optional string)
* `chunks` (array of `{ heading, text, token_count }`)
* `rendering_method` ("http" | "browser")
* `note` (optional string, e.g. `cache_hit`, `rendered_with_browser`)

### 3.2 SSRF and URL Validation
**FR-WF-04:** Only `http://` and `https://` schemes are allowed.

**FR-WF-05:** The validator MUST block:
* `localhost` and loopback hostnames
* Private and reserved IP ranges (IPv4 and IPv6)
* IPv4-mapped IPv6 private addresses

**FR-WF-06:** DNS resolution MUST be performed and all resolved IPs MUST pass SSRF checks.

**FR-WF-07:** Redirects MUST be followed manually and re-validated per hop. Maximum hops MUST default to 5.

### 3.3 robots.txt
**FR-WF-08:** WebFetch MUST fetch and cache robots.txt per domain and enforce disallow rules.

**FR-WF-09:** Missing robots.txt MUST be treated as allow-all.

### 3.4 Fetch and Rendering
**FR-WF-10:** HTTP mode MUST use a fixed user-agent string and a request timeout (default 20s).

**FR-WF-11:** Browser mode MUST use headless Chromium when available. If Chromium is unavailable and browser mode is required, the tool MUST return a clear error.

**FR-WF-12:** WebFetch MUST implement HTTP-first rendering with:
* Forced browser rendering when `force_browser=true`
* Domain whitelist for known JS-heavy sites
* Heuristic fallback to browser rendering when SPA indicators are detected

### 3.5 Extraction and Chunking
**FR-WF-13:** HTML content MUST be converted to Markdown with boilerplate removal and whitespace normalization.

**FR-WF-14:** Chunking MUST be token-aware using `cl100k_base` tokenizer semantics. Default chunk size MUST be 600 tokens unless overridden.

**FR-WF-15:** Chunking SHOULD respect Markdown headings as preferred boundaries and include the most recent heading in `FetchChunk.heading`.

### 3.6 Caching
**FR-WF-16:** The tool MUST cache fetched content on disk and separate HTTP vs browser renders by cache key.

**FR-WF-17:** When `no_cache=true`, the tool MUST bypass cache read but SHOULD still write the fresh result to cache.

### 3.7 Errors and Remediation
**FR-WF-18:** Errors MUST be returned as tool error payloads with user-actionable messages.

**FR-WF-19:** The tool SHOULD map common failures to stable error codes such as:
`ssrf_blocked`, `robots_disallowed`, `browser_unavailable`, `http_404`, `timeout`, `network`, `unknown`.

---

## 4. Non-Functional Requirements

### 4.1 Security
| Requirement | Specification |
| --- | --- |
| NFR-WF-SEC-01 | SSRF protection MUST validate scheme, host, DNS, and redirects |
| NFR-WF-SEC-02 | robots.txt MUST be enforced for the configured user-agent |
| NFR-WF-SEC-03 | Output MUST be treated as untrusted input and labeled accordingly |

### 4.2 Performance
| Requirement | Specification |
| --- | --- |
| NFR-WF-PERF-01 | HTTP fetches SHOULD complete under 20s typical conditions |
| NFR-WF-PERF-02 | Chunking SHOULD be linear in content length |

### 4.3 Reliability
| Requirement | Specification |
| --- | --- |
| NFR-WF-REL-01 | Cache reads/writes MUST be atomic |
| NFR-WF-REL-02 | Failures MUST not crash the tool loop |

---

## 5. Configuration

```toml
[webfetch]
enabled = false
user_agent = "forge-webfetch/1.0"
timeout_seconds = 20
max_redirects = 5
default_max_chunk_tokens = 600
cache_dir = "${TEMP}/forge-webfetch"
robots_cache_entries = 1024
```

```toml
[webfetch.browser]
enabled = true
chromium_path = ""
network_idle_ms = 20000
block_resources = ["image", "font", "media"]
```

```toml
[webfetch.security]
block_private_ips = true
block_loopback = true
block_link_local = true
```

---

## 6. Verification Requirements

### 6.1 Unit Tests
| Test ID | Description |
| --- | --- |
| T-WF-SSRF-01 | Reject non-http schemes |
| T-WF-SSRF-02 | Reject localhost and private IPs |
| T-WF-SSRF-03 | Reject redirects to private IPs |
| T-WF-ROB-01 | Disallow robots.txt blocked paths |
| T-WF-CHK-01 | Chunk sizes do not exceed max tokens |
| T-WF-CCH-01 | Cache hit returns prior content |

### 6.2 Integration Tests
| Test ID | Description |
| --- | --- |
| IT-WF-HTTP-01 | Fetch and extract Markdown via HTTP |
| IT-WF-BR-01 | Browser render succeeds when forced |
| IT-WF-ERR-01 | Browser-unavailable error is mapped correctly |

