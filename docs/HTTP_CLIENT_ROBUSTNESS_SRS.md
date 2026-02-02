# HTTP Client Robustness SRS

Production-grade HTTP client requirements synthesized from official Anthropic and OpenAI SDKs.

Both providers use [Stainless](https://www.stainlessapi.com/) for SDK generation, resulting in nearly identical HTTP client patterns across all official SDKs.

## SDK Analysis

### Source Code Reviewed

#### Anthropic SDKs

| SDK | Key Files |
|-----|-----------|
| Python | `_base_client.py`, `_constants.py`, `_streaming.py` |
| Java | `OkHttpClient.kt`, `RetryingHttpClient.kt`, `Timeout.kt` |
| Go | `requestconfig.go`, `ssestream.go`, `constants.go` |
| TypeScript | `client.ts`, `streaming.ts`, `constants.ts` |

#### OpenAI SDKs

| SDK | Key Files |
|-----|-----------|
| Python | `_base_client.py`, `_constants.py`, `_streaming.py` |
| Node/TS | `src/client.ts`, `src/core/streaming.ts` |
| Go | `internal/requestconfig/requestconfig.go`, `packages/ssestream/ssestream.go` |
| Java | `RetryingHttpClient.kt`, `SseHandler.kt`, `Timeout.kt` |
| .NET | Delegates to `System.ClientModel` (Azure SDK patterns) |

#### Community SDKs

| SDK | Key Files |
|-----|-----------|
| async-openai (Rust) | `client.rs`, `config.rs` - uses `backoff` crate |

### Cross-Provider Feature Matrix

| Feature | Anthropic | OpenAI | async-openai | Forge |
|---------|-----------|--------|--------------|-------|
| Max Retries | 2 | 2 (.NET: 3) | unlimited | 0 (API), 3 (summarize) |
| Initial Delay | 0.5s | 0.5s | 1ms | N/A |
| Max Delay | 8s | 8s | 32s | N/A |
| Jitter | ±25% | ±25% | ±10% | 0-200ms |
| Retry-After | ✅ 3-tier | ✅ 3-tier | ❌ | ❌ |
| x-should-retry | ✅ | ✅ | ❌ | ❌ |
| Retry Count Header | ✅ | ✅ | ❌ | ❌ |
| Idempotency-Key | ✅ | ✅ (not Go) | ❌ | ❌ |
| Connection Pool | 1000/100 (Python) | 1000/100 (Python) | delegated | default |
| Connect Timeout | 5s (Python) | 5s (Python) | none | 30s |
| Request Timeout | 10min | 10min | none | none |
| TCP Keepalive | Python only | ❌ | ❌ | ❌ |
| HTTP/2 Ping | Java only | ❌ | ❌ | ❌ |
| Idle Detection | ❌ | ❌ | ❌ | ✅ 60s |

---

## Specification

### 1. TCP Keepalive

**Source**: Anthropic Python SDK (`_base_client.py:845-869`)

**Note**: Only the Anthropic Python SDK configures TCP keepalive. OpenAI SDKs and other Anthropic SDKs delegate to HTTP library defaults.

**Specification**:
```
SO_KEEPALIVE = enabled
TCP_KEEPIDLE = 60 seconds (time before first probe)
TCP_KEEPINTVL = 60 seconds (interval between probes)
TCP_KEEPCNT = 5 (max failed probes before disconnect)
```

**Rust Implementation**:
```rust
reqwest::Client::builder()
    .tcp_keepalive(Duration::from_secs(60))
```

**Rationale**: Detects half-open connections where server has died but client hasn't received RST. Critical for long-running streaming requests that may idle during model thinking.

---

### 2. Connection Pool Configuration

**Source**: Anthropic/OpenAI Python SDKs (`_constants.py`)

**Note**: Both providers use identical httpx connection limits. Other language SDKs delegate to their HTTP library defaults.

**Specification**:
```
max_connections = 1000 (total)
max_keepalive_connections = 100 (idle per host)
pool_idle_timeout = 90 seconds
```

**Rust Implementation**:
```rust
reqwest::Client::builder()
    .pool_max_idle_per_host(100)
    .pool_idle_timeout(Duration::from_secs(90))
```

**Rationale**: Prevents unbounded connection accumulation. Ensures efficient reuse for same provider.

---

### 3. HTTP/2 Ping Interval

**Source**: Anthropic Java SDK (`OkHttpClient.kt:262`)

**Note**: Only the Anthropic Java SDK configures HTTP/2 pings. OpenAI SDKs do not configure this.

**Specification**:
```
ping_interval = 60 seconds
ping_timeout = 20 seconds
ping_while_idle = true
```

**Rust Implementation**:
```rust
reqwest::Client::builder()
    .http2_keep_alive_interval(Duration::from_secs(60))
    .http2_keep_alive_timeout(Duration::from_secs(20))
    .http2_keep_alive_while_idle(true)
```

**Rationale**: Keeps HTTP/2 connections alive through load balancers and firewalls. Important for long-running streaming requests.

---

### 4. Retry with Exponential Backoff

**Source**: All Anthropic and OpenAI SDKs (identical Stainless-generated implementation)

**Specification**:
```
max_retries = 2 (3 total attempts)
initial_delay = 500ms
max_delay = 8 seconds
jitter = ±25% (multiply by 1 - random(0, 0.25))
formula = min(0.5 * 2^attempt, 8.0) * jitter
```

**Retryable Conditions**:

| Condition | Retry? |
|-----------|--------|
| HTTP 408 (Request Timeout) | Yes |
| HTTP 409 (Conflict/Lock Timeout) | Yes |
| HTTP 429 (Rate Limit) | Yes |
| HTTP 5xx (Server Error) | Yes |
| Connection error / IOException | Yes |
| `x-should-retry: true` header | Yes |
| `x-should-retry: false` header | No (override) |

**Retry-After Header Parsing**:

| Header | Parsing |
|--------|---------|
| `Retry-After-Ms` | Float milliseconds |
| `Retry-After` (numeric) | Float seconds |
| `Retry-After` (date) | RFC 1123 HTTP-date, compute delta |
| Bounds | Only use if 0 < delay < 60 seconds |

**Rust Implementation**:
```rust
pub struct RetryConfig {
    pub max_retries: u32,
    pub initial_delay: Duration,
    pub max_delay: Duration,
    pub jitter_factor: f64,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            max_retries: 2,
            initial_delay: Duration::from_millis(500),
            max_delay: Duration::from_secs(8),
            jitter_factor: 0.25,
        }
    }
}

fn calculate_retry_delay(
    attempt: u32,
    config: &RetryConfig,
    headers: Option<&HeaderMap>,
) -> Duration {
    // Check Retry-After headers first
    if let Some(delay) = parse_retry_after(headers) {
        if delay > Duration::ZERO && delay < Duration::from_secs(60) {
            return delay;
        }
    }
    // Exponential backoff with jitter
    let base = config.initial_delay.as_secs_f64() * 2.0_f64.powi(attempt as i32);
    let capped = base.min(config.max_delay.as_secs_f64());
    let jitter = 1.0 - rand::random::<f64>() * config.jitter_factor;
    Duration::from_secs_f64(capped * jitter)
}

fn should_retry(status: StatusCode, headers: &HeaderMap) -> bool {
    // Explicit header override
    if let Some(val) = headers.get("x-should-retry") {
        if val == "true" { return true; }
        if val == "false" { return false; }
    }
    matches!(status.as_u16(), 408 | 409 | 429 | 500..=599)
}
```

**Rationale**: All four SDKs implement identical logic. Currently Forge only retries summarization calls; LLM API calls fail immediately on transient errors.

---

### 5. Timeout Configuration

**Source**: Java SDK (`Timeout.kt`)

**Specification**:

| Timeout Type | Default | Description |
|--------------|---------|-------------|
| Connect | 30 seconds | TCP handshake + TLS negotiation |
| Read | 60 seconds | Between data packets from server |
| Write | 60 seconds | Sending request body |
| Request | 10 minutes | Total operation time |

**Streaming Behavior**:
- Non-streaming: Apply request timeout (10 min)
- Streaming: No total timeout; rely on idle detection (60s between chunks)

**Rust Implementation**:
```rust
pub struct Timeout {
    pub connect: Duration,
    pub read: Option<Duration>,
    pub request: Option<Duration>,
}

impl Timeout {
    pub fn for_streaming() -> Self {
        Self {
            connect: Duration::from_secs(30),
            read: None,    // Handled by idle detection
            request: None, // No total timeout for streams
        }
    }

    pub fn for_request(total: Duration) -> Self {
        Self {
            connect: Duration::from_secs(30),
            read: Some(Duration::from_secs(60)),
            request: Some(total),
        }
    }
}
```

---

### 6. Request Headers

**Source**: All Anthropic and OpenAI SDKs

#### Retry Headers

| Header | Value | Purpose |
|--------|-------|---------|
| `X-Stainless-Retry-Count` | `0`, `1`, `2`, ... | Current retry attempt |
| `X-Stainless-Timeout` | Seconds (integer) | Client timeout for server optimization |
| `Idempotency-Key` | `stainless-{lang}-retry-{uuid}` | Safe retry deduplication |

#### Idempotency Key Formats by Language

| Language | Format |
|----------|--------|
| Python | `stainless-python-retry-{uuid}` |
| Node/TS | `stainless-node-retry-{uuid}` |
| Java | `stainless-java-retry-{uuid}` |
| Go | Not implemented |
| Rust (proposed) | `stainless-rust-retry-{uuid}` |

#### Platform Headers (sent on all requests)

| Header | Example Value |
|--------|---------------|
| `X-Stainless-Lang` | `python`, `js`, `go`, `java` |
| `X-Stainless-Package-Version` | `1.0.0` |
| `X-Stainless-OS` | `Linux`, `Darwin`, `Windows` |
| `X-Stainless-Arch` | `x64`, `arm64` |
| `X-Stainless-Runtime` | `CPython`, `node`, `go` |
| `X-Stainless-Runtime-Version` | `3.11.0`, `20.0.0` |

**Rust Implementation**:
```rust
fn add_retry_headers(
    builder: RequestBuilder,
    attempt: u32,
    idempotency_key: &str,
    timeout: Option<Duration>,
) -> RequestBuilder {
    let mut builder = builder
        .header("X-Stainless-Retry-Count", attempt.to_string());

    if let Some(t) = timeout {
        builder = builder.header("X-Stainless-Timeout", t.as_secs().to_string());
    }

    if attempt == 0 {
        builder = builder.header("Idempotency-Key", idempotency_key);
    }

    builder
}
```

---

### 7. Non-Streaming Token Limits (Anthropic Only)

**Source**: Anthropic SDKs (`constants.go`, `_constants.py`, `constants.ts`)

**Note**: OpenAI SDKs do not enforce non-streaming token limits.

**Specification**:

| Model Pattern | Max Non-Streaming Tokens |
|---------------|-------------------------|
| `claude-opus-4-*` | 8,192 |
| `claude-opus-4-1-*` | 8,192 |
| Other models | No limit |

**Timeout Calculation**:
```rust
fn calculate_nonstreaming_timeout(max_tokens: u32, model: &str) -> Result<Duration, Error> {
    const MAX_TIME_SECS: u64 = 3600;  // 1 hour
    const DEFAULT_TIME_SECS: u64 = 600;  // 10 minutes

    let expected_secs = (MAX_TIME_SECS as f64 * max_tokens as f64 / 128_000.0) as u64;

    if expected_secs > DEFAULT_TIME_SECS {
        return Err(Error::StreamingRequired);
    }

    // Check model-specific limits
    if model.contains("opus-4") && max_tokens > 8192 {
        return Err(Error::StreamingRequired);
    }

    Ok(Duration::from_secs(DEFAULT_TIME_SECS))
}
```

**Rationale**: Forces streaming for large outputs to prevent timeout failures.

---

### 8. SSE Event Handling

Both providers use Server-Sent Events for streaming, with provider-specific event types.

#### Common Patterns

| Pattern | Both Providers |
|---------|----------------|
| Termination signal | `[DONE]` marker in data field |
| Error detection | `error` field in JSON payload |
| Event boundaries | Double newline (`\n\n`) |
| Comment lines | Lines starting with `:` ignored |

#### Anthropic SSE Events

| Event Type | Description |
|------------|-------------|
| `message_start` | Message metadata, usage info |
| `message_delta` | Stop reason, final usage |
| `message_stop` | Stream complete |
| `content_block_start` | New content block with type |
| `content_block_delta` | Text or tool use delta |
| `content_block_stop` | Content block complete |
| `ping` | Keepalive (ignore, resets idle timer) |
| `error` | API error with type/message |

#### OpenAI SSE Events

| Event Type | Description |
|------------|-------------|
| (no event field) | Chat completion chunk |
| `thread.run.created` | Assistants API: run started |
| `thread.run.completed` | Assistants API: run finished |
| `thread.message.delta` | Assistants API: message chunk |
| `error` | API error |

**Current Status**: Forge implements Anthropic SSE correctly in `process_sse_stream()`. OpenAI streaming uses simpler event structure (no event type field for chat completions).

---

## Implementation Plan

### Priority Matrix

| Priority | Feature | Effort | Impact |
|----------|---------|--------|--------|
| P0 | TCP Keepalive | 1 line | High - prevents dead connection hangs |
| P0 | Retry with Backoff | ~100 LOC | High - handles transient failures |
| P0 | Connection Pool Limits | 2 lines | Medium - prevents resource exhaustion |
| P1 | HTTP/2 Ping Interval | 3 lines | Medium - keeps long streams alive |
| P1 | Retry Headers | ~20 LOC | Low - server-side optimization |
| P1 | Idempotency Keys | ~10 LOC | Low - safe retries |
| P2 | Token Limits | ~30 LOC | Low - edge case protection |
| P2 | Granular Timeouts | ~50 LOC | Low - more control |

### Files to Modify

| File | Changes |
|------|---------|
| `providers/src/lib.rs` | HTTP client config, retry wrapper, header injection |
| `providers/Cargo.toml` | Add `rand` crate for jitter |
| `context/src/summarization.rs` | Unify retry logic with shared implementation |

### Testing Strategy

1. **Unit Tests**
   - `calculate_retry_delay()` with various attempts
   - `should_retry()` with all status codes and headers
   - `parse_retry_after()` with various header formats

2. **Integration Tests** (wiremock)
   - Mock server returning 429 with Retry-After
   - Mock server returning 503 then 200
   - Mock server with x-should-retry headers

3. **Manual Testing**
   - Network disconnect during stream
   - Rate limit from real API

---

## What Forge Already Does Better

| Feature | Forge | Anthropic SDKs | OpenAI SDKs |
|---------|-------|----------------|-------------|
| Idle stream detection | ✅ 60s timeout | ❌ None | ❌ None |
| Error body size cap | ✅ 32 KiB | ❌ Unbounded | ❌ Unbounded |
| SSE buffer limit | ✅ 4 MiB | ❌ Varies | ❌ Varies |
| Parse error threshold | ✅ 3 failures abort | ❌ Varies | ❌ Varies |

Forge's idle stream detection is ahead of all official SDKs from both providers.

---

## Community SDK Comparison

The **async-openai** Rust crate takes a different approach:

| Feature | async-openai | Official SDKs |
|---------|--------------|---------------|
| Retry library | `backoff` crate | Custom implementation |
| Initial delay | 1ms | 500ms |
| Max delay | 32s | 8s |
| Jitter | ±10% | ±25% |
| Retry-After parsing | ❌ | ✅ |
| x-should-retry | ❌ | ✅ |
| Idempotency keys | ❌ | ✅ |
| 429 handling | ✅ (except insufficient_quota) | ✅ |

**Recommendation**: Forge should follow the official SDK patterns rather than async-openai's approach for consistency with provider expectations.

---

## Out of Scope

Features missing from all official SDKs (genuinely hard problems):

| Feature | Status |
|---------|--------|
| Hedged requests | Not implemented anywhere |
| Partial stream recovery | Not implemented anywhere |
| Circuit breaker | Not implemented anywhere |
| Adaptive timeouts | Not implemented anywhere |
