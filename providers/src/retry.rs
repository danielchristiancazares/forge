//! HTTP retry policy with exponential backoff.
//!
//! Implements retry behavior matching official Anthropic/OpenAI SDKs (Stainless-generated).
//!
//! # Retry Policy (REQ-4)
//!
//! - Max retries: 2 (3 total attempts)
//! - Initial delay: 500ms
//! - Max delay: 8 seconds
//! - Jitter: down-jitter up to 25% (multiplier in [0.75, 1.0])
//!
//! # Retryable Conditions
//!
//! - HTTP 408, 409, 429, 5xx
//! - Connection errors
//! - `x-should-retry: true` forces retry
//! - `x-should-retry: false` forbids retry
//!
//! # Headers (REQ-6)
//!
//! - `X-Stainless-Retry-Count`: 0 for initial, 1+ for retries
//! - `Idempotency-Key`: `stainless-retry-{uuid}`, same across all attempts
//! - `X-Stainless-Timeout`: request timeout in seconds (non-streaming only)

use std::time::Duration;

use reqwest::{RequestBuilder, Response, StatusCode, header::HeaderMap};
use uuid::Uuid;

/// Retry configuration matching official SDK defaults.
#[derive(Debug, Clone)]
pub struct RetryConfig {
    /// Maximum number of retries (not counting initial request).
    pub max_retries: u32,
    /// Initial backoff delay before first retry.
    pub initial_delay: Duration,
    /// Maximum backoff delay.
    pub max_delay: Duration,
    /// Jitter factor for down-jitter (0.25 = up to 25% reduction).
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

/// Parse `Retry-After` or `Retry-After-Ms` headers.
///
/// Returns `Some(duration)` if a valid value is found and `0 < duration < 60s`.
/// Returns `None` if headers are missing, invalid, or out of range.
#[must_use]
pub fn parse_retry_after(headers: &HeaderMap) -> Option<Duration> {
    // Try Retry-After-Ms first (milliseconds, float)
    if let Some(val) = headers.get("retry-after-ms")
        && let Ok(s) = val.to_str()
        && let Ok(ms) = s.parse::<f64>()
    {
        let duration = Duration::from_secs_f64(ms / 1000.0);
        if duration > Duration::ZERO && duration < Duration::from_secs(60) {
            return Some(duration);
        }
    }

    // Try Retry-After (seconds, integer)
    if let Some(val) = headers.get("retry-after")
        && let Ok(s) = val.to_str()
        && let Ok(secs) = s.parse::<u64>()
    {
        let duration = Duration::from_secs(secs);
        if duration > Duration::ZERO && duration < Duration::from_secs(60) {
            return Some(duration);
        }
    }

    None
}

/// Determine if a response status is retryable.
///
/// Respects `x-should-retry` header override if present.
#[must_use]
pub fn should_retry(status: StatusCode, headers: &HeaderMap) -> bool {
    // Explicit header override
    if let Some(val) = headers.get("x-should-retry")
        && let Ok(s) = val.to_str()
    {
        if s.eq_ignore_ascii_case("true") {
            return true;
        }
        if s.eq_ignore_ascii_case("false") {
            return false;
        }
    }

    // Default retryable statuses
    matches!(
        status.as_u16(),
        408 | 409 | 429 | 500 | 502 | 503 | 504 | 520..=599
    )
}

/// Calculate retry delay with exponential backoff and jitter.
///
/// - `backoff_step`: 0 before first retry, 1 before second, etc.
/// - Respects `Retry-After` headers if present and valid.
#[must_use]
pub fn calculate_retry_delay(
    backoff_step: u32,
    config: &RetryConfig,
    headers: Option<&HeaderMap>,
) -> Duration {
    // Check Retry-After headers first
    if let Some(headers) = headers
        && let Some(delay) = parse_retry_after(headers)
    {
        return delay;
    }

    // Exponential backoff: initial_delay * 2^backoff_step
    let base = config.initial_delay.as_secs_f64() * 2.0_f64.powi(backoff_step as i32);
    let capped = base.min(config.max_delay.as_secs_f64());

    // Down-jitter: multiply by random factor in [1 - jitter_factor, 1.0]
    let jitter = 1.0 - rand::random::<f64>() * config.jitter_factor;
    Duration::from_secs_f64(capped * jitter)
}

/// Add retry-related headers to a request.
///
/// - `retry_count`: 0 for initial request, 1+ for retries
/// - `idempotency_key`: same UUID across all attempts
/// - `timeout`: request timeout (omit for streaming)
pub fn add_retry_headers(
    builder: RequestBuilder,
    retry_count: u32,
    idempotency_key: &str,
    timeout: Option<Duration>,
) -> RequestBuilder {
    let mut builder = builder
        .header("X-Stainless-Retry-Count", retry_count.to_string())
        .header("Idempotency-Key", idempotency_key);

    if let Some(t) = timeout {
        builder = builder.header("X-Stainless-Timeout", t.as_secs().to_string());
    }

    builder
}

#[must_use]
pub fn generate_idempotency_key() -> String {
    format!("stainless-retry-{}", Uuid::new_v4())
}

/// Outcome of a retry operation.
///
/// This is a sum type that structurally distinguishes success from failure,
/// ensuring callers cannot accidentally treat an error response as success.
#[derive(Debug)]
pub enum RetryOutcome {
    /// Request succeeded (2xx status).
    Success(Response),
    /// Request failed with an HTTP error after exhausting retries.
    /// The response is provided for error body inspection.
    HttpError(Response),
    /// Request failed with a connection/transport error after exhausting retries.
    ConnectionError {
        attempts: u32,
        source: reqwest::Error,
    },
    /// Request failed with a non-retryable connection error on first attempt.
    NonRetryable(reqwest::Error),
}

impl RetryOutcome {
    /// Returns the successful response, or an error description.
    ///
    /// Convenience method for callers that want simple error handling.
    pub fn into_response(self) -> Result<Response, String> {
        match self {
            Self::Success(r) => Ok(r),
            Self::HttpError(r) => Err(format!("HTTP error: {}", r.status())),
            Self::ConnectionError { attempts, source } => Err(format!(
                "connection error after {attempts} attempts: {source}"
            )),
            Self::NonRetryable(e) => Err(format!("request error: {e}")),
        }
    }

    /// Returns true if this is a successful response.
    #[must_use]
    pub const fn is_success(&self) -> bool {
        matches!(self, Self::Success(_))
    }
}

/// Send a request with automatic retries.
///
/// # Arguments
///
/// - `build_request`: Closure that builds the request. Called for each attempt.
/// - `timeout`: Optional request timeout (included in `X-Stainless-Timeout` header).
/// - `config`: Retry configuration.
///
/// # Retry Behavior
///
/// - Retries on connection errors and retryable HTTP statuses (408, 409, 429, 5xx).
/// - Respects `x-should-retry` header from server.
/// - Uses `Retry-After` or exponential backoff for delay.
/// - Sends consistent `Idempotency-Key` across all attempts.
///
/// # Returns
///
/// A `RetryOutcome` that structurally distinguishes:
/// - `Success`: 2xx response
/// - `HttpError`: non-2xx response after exhausting retries
/// - `ConnectionError`: transport failure after exhausting retries
/// - `NonRetryable`: transport failure on first attempt that cannot be retried
pub async fn send_with_retry<F>(
    build_request: F,
    timeout: Option<Duration>,
    config: &RetryConfig,
) -> RetryOutcome
where
    F: Fn() -> RequestBuilder,
{
    let idempotency_key = generate_idempotency_key();

    // Handle max_retries == 0 case: single attempt only
    if config.max_retries == 0 {
        return execute_single_attempt(&build_request, &idempotency_key, timeout, 0).await;
    }

    // Attempts 0 through max_retries-1: can retry on failure
    for retry_count in 0..config.max_retries {
        let request = add_retry_headers(build_request(), retry_count, &idempotency_key, timeout);

        match request.send().await {
            Ok(response) => {
                let status = response.status();
                let headers = response.headers().clone();

                if status.is_success() {
                    return RetryOutcome::Success(response);
                }

                if should_retry(status, &headers) {
                    let delay = calculate_retry_delay(retry_count, config, Some(&headers));
                    tracing::debug!(
                        status = %status,
                        retry_count = retry_count + 1,
                        delay_ms = delay.as_millis(),
                        "Retrying request after error status"
                    );
                    tokio::time::sleep(delay).await;
                    continue;
                }

                // Non-retryable HTTP status
                return RetryOutcome::HttpError(response);
            }
            Err(e) => {
                if is_retryable_error(&e) {
                    let delay = calculate_retry_delay(retry_count, config, None);
                    tracing::debug!(
                        error = %e,
                        retry_count = retry_count + 1,
                        delay_ms = delay.as_millis(),
                        "Retrying request after connection error"
                    );
                    tokio::time::sleep(delay).await;
                    continue;
                }

                // Non-retryable error
                if retry_count == 0 {
                    return RetryOutcome::NonRetryable(e);
                }
                return RetryOutcome::ConnectionError {
                    attempts: retry_count + 1,
                    source: e,
                };
            }
        }
    }

    // Final attempt (retry_count == max_retries): no more retries possible
    let request = add_retry_headers(
        build_request(),
        config.max_retries,
        &idempotency_key,
        timeout,
    );

    match request.send().await {
        Ok(response) => {
            if response.status().is_success() {
                RetryOutcome::Success(response)
            } else {
                RetryOutcome::HttpError(response)
            }
        }
        Err(e) => RetryOutcome::ConnectionError {
            attempts: config.max_retries + 1,
            source: e, // Error returned directly - no stashing
        },
    }
}

/// Helper for single-attempt case (max_retries == 0).
async fn execute_single_attempt<F>(
    build_request: &F,
    idempotency_key: &str,
    timeout: Option<Duration>,
    retry_count: u32,
) -> RetryOutcome
where
    F: Fn() -> RequestBuilder,
{
    let request = add_retry_headers(build_request(), retry_count, idempotency_key, timeout);

    match request.send().await {
        Ok(response) => {
            if response.status().is_success() {
                RetryOutcome::Success(response)
            } else {
                RetryOutcome::HttpError(response)
            }
        }
        Err(e) => RetryOutcome::NonRetryable(e),
    }
}

fn is_retryable_error(error: &reqwest::Error) -> bool {
    error.is_connect() || error.is_timeout() || error.is_request()
}

#[cfg(test)]
mod tests {
    use super::*;
    use reqwest::header::HeaderValue;

    #[test]
    fn test_parse_retry_after_ms() {
        let mut headers = HeaderMap::new();
        headers.insert("retry-after-ms", HeaderValue::from_static("1500"));
        assert_eq!(
            parse_retry_after(&headers),
            Some(Duration::from_millis(1500))
        );
    }

    #[test]
    fn test_parse_retry_after_seconds() {
        let mut headers = HeaderMap::new();
        headers.insert("retry-after", HeaderValue::from_static("5"));
        assert_eq!(parse_retry_after(&headers), Some(Duration::from_secs(5)));
    }

    #[test]
    fn test_parse_retry_after_out_of_range() {
        let mut headers = HeaderMap::new();
        headers.insert("retry-after", HeaderValue::from_static("120"));
        assert_eq!(parse_retry_after(&headers), None);

        headers.clear();
        headers.insert("retry-after", HeaderValue::from_static("0"));
        assert_eq!(parse_retry_after(&headers), None);
    }

    #[test]
    fn test_should_retry_status_codes() {
        let headers = HeaderMap::new();
        assert!(should_retry(StatusCode::TOO_MANY_REQUESTS, &headers)); // 429
        assert!(should_retry(StatusCode::INTERNAL_SERVER_ERROR, &headers)); // 500
        assert!(should_retry(StatusCode::BAD_GATEWAY, &headers)); // 502
        assert!(should_retry(StatusCode::SERVICE_UNAVAILABLE, &headers)); // 503
        assert!(should_retry(StatusCode::GATEWAY_TIMEOUT, &headers)); // 504
        assert!(should_retry(StatusCode::REQUEST_TIMEOUT, &headers)); // 408
        assert!(should_retry(StatusCode::CONFLICT, &headers)); // 409

        assert!(!should_retry(StatusCode::BAD_REQUEST, &headers)); // 400
        assert!(!should_retry(StatusCode::UNAUTHORIZED, &headers)); // 401
        assert!(!should_retry(StatusCode::NOT_FOUND, &headers)); // 404
    }

    #[test]
    fn test_should_retry_header_override() {
        let mut headers = HeaderMap::new();

        // Force retry on non-retryable status
        headers.insert("x-should-retry", HeaderValue::from_static("true"));
        assert!(should_retry(StatusCode::BAD_REQUEST, &headers));

        // Forbid retry on retryable status
        headers.clear();
        headers.insert("x-should-retry", HeaderValue::from_static("false"));
        assert!(!should_retry(StatusCode::TOO_MANY_REQUESTS, &headers));
    }

    #[test]
    fn test_calculate_retry_delay_bounds() {
        let config = RetryConfig::default();

        // First retry (backoff_step=0): base = 500ms
        // With jitter in [0.75, 1.0], delay should be in [375ms, 500ms]
        for _ in 0..100 {
            let delay = calculate_retry_delay(0, &config, None);
            assert!(delay >= Duration::from_millis(375));
            assert!(delay <= Duration::from_millis(500));
        }

        // Second retry (backoff_step=1): base = 1000ms
        // With jitter, delay should be in [750ms, 1000ms]
        for _ in 0..100 {
            let delay = calculate_retry_delay(1, &config, None);
            assert!(delay >= Duration::from_millis(750));
            assert!(delay <= Duration::from_millis(1000));
        }
    }

    #[test]
    fn test_calculate_retry_delay_respects_retry_after() {
        let config = RetryConfig::default();
        let mut headers = HeaderMap::new();
        headers.insert("retry-after", HeaderValue::from_static("3"));

        let delay = calculate_retry_delay(0, &config, Some(&headers));
        assert_eq!(delay, Duration::from_secs(3));
    }

    #[test]
    fn test_generate_idempotency_key() {
        let key1 = generate_idempotency_key();
        let key2 = generate_idempotency_key();
        assert!(key1.starts_with("stainless-retry-"));
        assert!(key2.starts_with("stainless-retry-"));
        assert_ne!(key1, key2);
    }
}

#[cfg(test)]
mod integration_tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    /// Fast retry config for tests (no delays).
    fn fast_retry_config() -> RetryConfig {
        RetryConfig {
            max_retries: 2,
            initial_delay: Duration::from_millis(1),
            max_delay: Duration::from_millis(10),
            jitter_factor: 0.0, // No jitter for deterministic tests
        }
    }

    #[tokio::test]
    async fn test_success_on_first_attempt() {
        let server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/test"))
            .respond_with(ResponseTemplate::new(200).set_body_string("ok"))
            .expect(1)
            .mount(&server)
            .await;

        let client = reqwest::Client::new();
        let url = format!("{}/test", server.uri());
        let config = fast_retry_config();

        let outcome = send_with_retry(|| client.get(&url), None, &config).await;

        match outcome {
            RetryOutcome::Success(response) => {
                assert_eq!(response.status(), StatusCode::OK);
                assert_eq!(response.text().await.unwrap(), "ok");
            }
            other => panic!("expected Success, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_retry_on_429_then_success() {
        let server = MockServer::start().await;
        let attempt = AtomicU32::new(0);

        Mock::given(method("GET"))
            .and(path("/test"))
            .respond_with(move |_: &wiremock::Request| {
                let n = attempt.fetch_add(1, Ordering::SeqCst);
                if n == 0 {
                    ResponseTemplate::new(429)
                } else {
                    ResponseTemplate::new(200).set_body_string("ok")
                }
            })
            .expect(2)
            .mount(&server)
            .await;

        let client = reqwest::Client::new();
        let url = format!("{}/test", server.uri());
        let config = fast_retry_config();

        let outcome = send_with_retry(|| client.get(&url), None, &config).await;

        match outcome {
            RetryOutcome::Success(response) => {
                assert_eq!(response.status(), StatusCode::OK);
            }
            other => panic!("expected Success, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_retry_on_500_then_success() {
        let server = MockServer::start().await;
        let attempt = AtomicU32::new(0);

        Mock::given(method("GET"))
            .and(path("/test"))
            .respond_with(move |_: &wiremock::Request| {
                let n = attempt.fetch_add(1, Ordering::SeqCst);
                if n == 0 {
                    ResponseTemplate::new(500)
                } else {
                    ResponseTemplate::new(200).set_body_string("ok")
                }
            })
            .expect(2)
            .mount(&server)
            .await;

        let client = reqwest::Client::new();
        let url = format!("{}/test", server.uri());
        let config = fast_retry_config();

        let outcome = send_with_retry(|| client.get(&url), None, &config).await;

        match outcome {
            RetryOutcome::Success(response) => {
                assert_eq!(response.status(), StatusCode::OK);
            }
            other => panic!("expected Success, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_exhausts_retries_returns_http_error() {
        let server = MockServer::start().await;

        // Always return 503
        Mock::given(method("GET"))
            .and(path("/test"))
            .respond_with(ResponseTemplate::new(503))
            .expect(3) // Initial + 2 retries
            .mount(&server)
            .await;

        let client = reqwest::Client::new();
        let url = format!("{}/test", server.uri());
        let config = fast_retry_config();

        let outcome = send_with_retry(|| client.get(&url), None, &config).await;

        // After exhausting retries, returns HttpError variant
        match outcome {
            RetryOutcome::HttpError(response) => {
                assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
            }
            other => panic!("expected HttpError, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_non_retryable_status_returns_http_error_immediately() {
        let server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/test"))
            .respond_with(ResponseTemplate::new(400).set_body_string("bad request"))
            .expect(1) // Only one attempt - no retries
            .mount(&server)
            .await;

        let client = reqwest::Client::new();
        let url = format!("{}/test", server.uri());
        let config = fast_retry_config();

        let outcome = send_with_retry(|| client.get(&url), None, &config).await;

        match outcome {
            RetryOutcome::HttpError(response) => {
                assert_eq!(response.status(), StatusCode::BAD_REQUEST);
            }
            other => panic!("expected HttpError, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_x_should_retry_false_prevents_retry() {
        let server = MockServer::start().await;

        // 429 is normally retryable, but x-should-retry: false overrides
        Mock::given(method("GET"))
            .and(path("/test"))
            .respond_with(ResponseTemplate::new(429).insert_header("x-should-retry", "false"))
            .expect(1) // Only one attempt
            .mount(&server)
            .await;

        let client = reqwest::Client::new();
        let url = format!("{}/test", server.uri());
        let config = fast_retry_config();

        let outcome = send_with_retry(|| client.get(&url), None, &config).await;

        match outcome {
            RetryOutcome::HttpError(response) => {
                assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
            }
            other => panic!("expected HttpError, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_x_should_retry_true_forces_retry() {
        let server = MockServer::start().await;
        let attempt = AtomicU32::new(0);

        Mock::given(method("GET"))
            .and(path("/test"))
            .respond_with(move |_: &wiremock::Request| {
                let n = attempt.fetch_add(1, Ordering::SeqCst);
                if n == 0 {
                    // 400 is not normally retryable, but x-should-retry: true forces it
                    ResponseTemplate::new(400).insert_header("x-should-retry", "true")
                } else {
                    ResponseTemplate::new(200).set_body_string("ok")
                }
            })
            .expect(2)
            .mount(&server)
            .await;

        let client = reqwest::Client::new();
        let url = format!("{}/test", server.uri());
        let config = fast_retry_config();

        let outcome = send_with_retry(|| client.get(&url), None, &config).await;

        match outcome {
            RetryOutcome::Success(response) => {
                assert_eq!(response.status(), StatusCode::OK);
            }
            other => panic!("expected Success, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_sends_retry_headers() {
        let server = MockServer::start().await;
        let attempt = AtomicU32::new(0);

        Mock::given(method("GET"))
            .and(path("/test"))
            .respond_with(move |req: &wiremock::Request| {
                let n = attempt.fetch_add(1, Ordering::SeqCst);

                // Verify retry headers
                let retry_count = req
                    .headers
                    .get("X-Stainless-Retry-Count")
                    .and_then(|v| v.to_str().ok())
                    .and_then(|s| s.parse::<u32>().ok())
                    .unwrap_or(999);

                let has_idempotency_key = req
                    .headers
                    .get("Idempotency-Key")
                    .and_then(|v| v.to_str().ok())
                    .is_some_and(|s| s.starts_with("stainless-retry-"));

                assert_eq!(retry_count, n, "retry count should match attempt number");
                assert!(has_idempotency_key, "should have idempotency key");

                if n == 0 {
                    ResponseTemplate::new(429)
                } else {
                    ResponseTemplate::new(200)
                }
            })
            .expect(2)
            .mount(&server)
            .await;

        let client = reqwest::Client::new();
        let url = format!("{}/test", server.uri());
        let config = fast_retry_config();

        let outcome = send_with_retry(|| client.get(&url), None, &config).await;

        assert!(outcome.is_success(), "expected Success");
    }

    #[tokio::test]
    async fn test_idempotency_key_consistent_across_retries() {
        let server = MockServer::start().await;
        let keys: std::sync::Arc<std::sync::Mutex<Vec<String>>> =
            std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let keys_clone = keys.clone();

        Mock::given(method("GET"))
            .and(path("/test"))
            .respond_with(move |req: &wiremock::Request| {
                let key = req
                    .headers
                    .get("Idempotency-Key")
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or("")
                    .to_string();

                keys_clone.lock().unwrap().push(key);

                if keys_clone.lock().unwrap().len() < 3 {
                    ResponseTemplate::new(500)
                } else {
                    ResponseTemplate::new(200)
                }
            })
            .expect(3)
            .mount(&server)
            .await;

        let client = reqwest::Client::new();
        let url = format!("{}/test", server.uri());
        let config = fast_retry_config();

        let _ = send_with_retry(|| client.get(&url), None, &config).await;

        let collected_keys = keys.lock().unwrap();
        assert_eq!(collected_keys.len(), 3);
        // All keys should be the same
        assert_eq!(collected_keys[0], collected_keys[1]);
        assert_eq!(collected_keys[1], collected_keys[2]);
    }

    #[tokio::test]
    async fn test_timeout_header_included_when_provided() {
        let server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/test"))
            .respond_with(|req: &wiremock::Request| {
                let timeout = req
                    .headers
                    .get("X-Stainless-Timeout")
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or("");

                assert_eq!(timeout, "30", "timeout should be 30 seconds");
                ResponseTemplate::new(200)
            })
            .expect(1)
            .mount(&server)
            .await;

        let client = reqwest::Client::new();
        let url = format!("{}/test", server.uri());
        let config = fast_retry_config();

        let outcome =
            send_with_retry(|| client.get(&url), Some(Duration::from_secs(30)), &config).await;

        assert!(outcome.is_success(), "expected Success");
    }
}
