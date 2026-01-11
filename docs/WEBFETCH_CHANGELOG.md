# WebFetch Tool - Change Log

Version history for [WEBFETCH_SRD.md](./WEBFETCH_SRD.md).

---

## 0.6 GPT-5.2p secondary review remediation (v2.4) (2026-01-10)
* **Status:** Addressed second-round implementation review findings
* **WebSocket handling (BLOCKING):** Added FR-WF-BSSR-WS-01 clarifying Option A (CDP Fetch) cannot proxy WebSockets—must block or use Option B (proxy)
* **Content-Encoding (BLOCKING):** Added FR-WF-BSSR-BODY-01 specifying response body handling for browser subrequest fulfillment (decompressed vs raw bytes + header adjustment)
* **Empty Allow/Disallow:** Added FR-WF-ROBOTS-EMPTY-01 per RFC 9309—empty Disallow allows all, empty Allow matches nothing
* **robots.txt BOM:** Added FR-WF-ROBOTS-BOM-01 specifying UTF-8 BOM stripping and non-UTF-8 BOM → allow-all
* **Empty extraction result:** Added FR-WF-EXT-EMPTY-02—if all root candidates empty, return `extraction_failed` (not empty chunks)
* **Browser unavailable definition:** FR-WF-11a now explicitly includes `browser.enabled=false` as "unavailable"
* **HTML sniffing BOM:** FR-WF-10h now strips UTF-8 BOM and leading whitespace before Content-Type sniffing
* **Timestamp precision:** Added FR-WF-CCH-TS-01 specifying RFC3339 with second precision only (no fractional seconds)
* **fetched_at timing:** Added FR-WF-CCH-TS-02 specifying completion time (not request start)
* **DNS error details:** FR-WF-DNS-01 now specifies first-failed-attempt error with `details.attempted_ips`
* **Title normalization:** FR-WF-13e now specifies whitespace normalization (trim + collapse)
* **Heading normalization:** FR-WF-CHK-HEAD-01 now specifies whitespace normalization (trim + collapse)
* **Removed orphan error code:** Removed `cache_read_failed` from error registry (FR-WF-CCH-READ-01 treats as cache miss, never returns error)
* **Tests:** Added T-WF-ROB-23/24/25/26 for empty rules and BOM; T-WF-EXT-08/09/10 for extraction edge cases; IT-WF-BR-14/15 for WebSocket and Content-Encoding

## 0.5 GPT-5.2p implementation review remediation (v2.3) (2026-01-10)
* **Status:** Addressed additional implementation review findings from GPT-5.2 Pro
* **Robots redirect (BLOCKING):** FR-WF-ROBOTS-REDIR-02 now explicitly allows http→https scheme upgrades while requiring same-host/port
* **Browser subrequests:** Added FR-WF-BSSR-SCHEME-01 specifying only http(s) subrequests permitted; data:/file:/blob: blocked via CDP
* **IP literal handling:** Added FR-WF-06-LITERAL specifying DNS skip for IP literals with direct SSRF validation
* **Timeout phases:** Added FR-WF-ERR-TIMEOUT-PHASE with canonical phase enum values (dns, connect, tls, request, response, redirect, browser_navigation, browser_network_idle, robots)
* **Error precedence:** Added FR-WF-04-PREC specifying URL validation error order (parse → scheme → userinfo → IPv6 zone → numeric IP)
* **Robots wildcard:** Added FR-WF-ROBOTS-WILDCARD-01 specifying `*` group loses tie-breaks against named groups
* **Budget scope:** Added FR-WF-BROWSER-BUDGET-02 clarifying main document counts toward subresource byte budget
* **Empty root:** Added FR-WF-EXT-EMPTY-01 defining "empty after boilerplate removal" as <50 Unicode scalar values
* **List-item splitting:** Added FR-WF-CHK-LIST-03 specifying oversized list items become standalone paragraphs (no bullet)
* **Cache eviction:** Added FR-WF-CCH-EVICT-01 with dual-limit eviction algorithm (interleaved LRU until both limits satisfied)
* **Unexpected status:** Added FR-WF-07a1 specifying handling for 1xx, 204, 304, and non-redirect 3xx responses
* **Config validation:** Added FR-WF-BLOCKRES-02 requiring CDP resource type validation at config load time
* **min_extracted_chars:** Clarified unit as Unicode scalar values (not bytes or grapheme clusters)
* **Appendix F:** Added Error Code Precedence reference with canonical ordering
* **Appendix G:** Added Browser Subrequest Validation Pipeline with CDP interception pseudocode
* **Tests:** Added T-WF-ROB-18/19/20 for robots http→https redirect scenarios; IT-WF-BR-11/12/13 for blocked subrequest schemes

## 0.4 GPT-5.2p compatibility review remediation (2026-01-10)
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

## 0.3 Implementation-readiness remediation (2026-01-10)
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

## 0.2 Comprehensive specification update (2026-01-10)
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

## 0.1 Initial draft (2026-01-08)
* Initial requirements for a WebFetch tool based on `../tools` WebFetch module.
