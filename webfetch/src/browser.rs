//! Headless Chromium rendering with CDP request interception.
//!
//! This module implements browser-mode rendering per `WEBFETCH_SRD.md`:
//! - Chromium launch with isolated user-data-dir (FR-WF-BROWSER-ISO-01)
//! - CDP Fetch interception for SSRF validation and IP pinning (FR-WF-11b)
//! - Resource blocking and method restrictions (FR-WF-11g, FR-WF-BROWSER-METHOD-01)
//! - Network-idle waiting (FR-WF-11e/11f)
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use chromiumoxide::Page;
use chromiumoxide::browser::{Browser, BrowserConfig};
use chromiumoxide::cdp::browser_protocol::fetch;
use chromiumoxide::cdp::browser_protocol::network;
use chromiumoxide::cdp::browser_protocol::page;
use chromiumoxide::handler::viewport::Viewport;
use futures_util::StreamExt;
use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
};
use std::time::{Duration, Instant as StdInstant, SystemTime, UNIX_EPOCH};
use tokio::sync::{Mutex, mpsc};
use tokio::time::{Instant as TokioInstant, sleep};
use url::Url;

use crate::http;
use crate::resolved::{BrowserPolicy, ChromiumLocation, ResolvedConfig};
use crate::types::{ErrorCode, TimeoutPhase, WebFetchError};

/// Check if browser rendering is available.
///
/// Per FR-WF-11a, browser is unavailable if:
/// - `browser.enabled = false` in config
/// - Chromium executable not found
pub fn is_available(config: &ResolvedConfig) -> bool {
    let browser_config = match &config.browser {
        BrowserPolicy::Enabled(cfg) => cfg,
        BrowserPolicy::Disabled => return false,
    };

    match &browser_config.chromium_path {
        ChromiumLocation::Explicit(path) => path.exists(),
        ChromiumLocation::SearchPath => find_chromium().is_some(),
    }
}

/// Render a page using headless Chromium with SSRF-safe interception.
pub async fn render(url: &Url, config: &ResolvedConfig) -> Result<BrowserResponse, WebFetchError> {
    let browser_config = match &config.browser {
        BrowserPolicy::Enabled(cfg) => cfg,
        BrowserPolicy::Disabled => {
            return Err(WebFetchError::new(
                ErrorCode::BrowserUnavailable,
                "browser rendering not available",
                false,
            ));
        }
    };

    let chromium_path = match &browser_config.chromium_path {
        ChromiumLocation::Explicit(path) => {
            if path.exists() {
                path.clone()
            } else {
                return Err(WebFetchError::new(
                    ErrorCode::BrowserUnavailable,
                    "chromium executable not found",
                    false,
                ));
            }
        }
        ChromiumLocation::SearchPath => find_chromium().ok_or_else(|| {
            WebFetchError::new(
                ErrorCode::BrowserUnavailable,
                "chromium executable not found",
                false,
            )
        })?,
    };

    let profile = TempProfileDir::new()?;

    let mut launch_args = Vec::new();
    launch_args.push("--disable-gpu".to_string());
    launch_args.push("--no-first-run".to_string());
    launch_args.push("--no-default-browser-check".to_string());
    launch_args.push(format!("--user-agent={}", config.user_agent));

    let browser_cfg = BrowserConfig::builder()
        .new_headless_mode()
        .chrome_executable(chromium_path)
        .user_data_dir(&profile.path)
        .viewport(Viewport {
            width: 1280,
            height: 720,
            device_scale_factor: Some(1.0),
            emulating_mobile: false,
            is_landscape: false,
            has_touch: false,
        })
        .args(launch_args)
        .build()
        .map_err(|e| {
            WebFetchError::new(
                ErrorCode::BrowserUnavailable,
                format!("failed to configure chromium: {e}"),
                false,
            )
        })?;

    let (browser, mut handler) = Browser::launch(browser_cfg).await.map_err(|e| {
        WebFetchError::new(
            ErrorCode::BrowserUnavailable,
            format!("failed to launch chromium: {e}"),
            false,
        )
    })?;

    tokio::spawn(async move { while let Some(_event) = handler.next().await {} });

    let page = browser.new_page("about:blank").await.map_err(|e| {
        WebFetchError::new(
            ErrorCode::BrowserCrashed,
            format!("failed to create page: {e}"),
            true,
        )
    })?;

    // Enable Fetch interception for all requests.
    page.execute(fetch::EnableParams {
        patterns: Some(vec![fetch::RequestPattern {
            url_pattern: Some("*".to_string()),
            resource_type: None,
            request_stage: Some(fetch::RequestStage::Request),
        }]),
        handle_auth_requests: Some(false),
    })
    .await
    .map_err(|e| {
        WebFetchError::new(
            ErrorCode::BrowserCrashed,
            format!("failed to enable fetch interception: {e}"),
            true,
        )
    })?;

    // Enable network events for idle detection.
    page.execute(network::EnableParams::default())
        .await
        .map_err(|e| {
            WebFetchError::new(
                ErrorCode::BrowserCrashed,
                format!("failed to enable network events: {e}"),
                true,
            )
        })?;

    let state = Arc::new(InterceptState::new());
    let blocked_resource_types = normalize_blocked_types(&browser_config.blocked_resource_types);

    let (fatal_tx, mut fatal_rx) = mpsc::unbounded_channel::<WebFetchError>();

    // Track in-flight requests for network idle.
    spawn_network_tracker(&page, state.clone()).await?;

    // Spawn Fetch.requestPaused handler.
    spawn_request_handler(
        &page,
        state.clone(),
        config.clone(),
        blocked_resource_types,
        fatal_tx,
    )
    .await?;

    let deadline = TokioInstant::now() + browser_config.timeout;

    // Navigate to the target URL.
    let nav = page.goto(url.as_str());
    tokio::select! {
        res = tokio::time::timeout_at(deadline, nav) => {
            match res {
                Ok(Ok(_)) => {}
                Ok(Err(e)) => {
                    return Err(WebFetchError::new(
                        ErrorCode::BrowserCrashed,
                        format!("navigation failed: {e}"),
                        true,
                    ));
                }
                Err(_) => {
                    return Err(timeout_error(TimeoutPhase::BrowserNavigation, browser_config.timeout));
                }
            }
        }
        Some(err) = fatal_rx.recv() => {
            return Err(err);
        }
    }

    // Wait for network idle.
    tokio::select! {
        res = wait_for_network_idle(state.clone(), deadline) => {
            res?;
        }
        Some(err) = fatal_rx.recv() => {
            return Err(err);
        }
    }

    // Extract DOM
    let html = extract_outer_html(&page).await?;
    let mut dom_truncated = false;
    let mut final_html = html;
    if final_html.len() > browser_config.max_rendered_dom_bytes as usize {
        final_html = truncate_utf8(&final_html, browser_config.max_rendered_dom_bytes as usize);
        dom_truncated = true;
    }
    let title = extract_title(&final_html);

    Ok(BrowserResponse {
        final_url: url.clone(),
        html: final_html,
        title,
        dom_truncated,
        blocked_non_get: state.blocked_non_get.load(Ordering::Relaxed),
    })
}

/// Browser rendering response.
#[derive(Debug)]
pub struct BrowserResponse {
    /// Final URL after navigation.
    pub final_url: Url,
    /// Rendered DOM as HTML string.
    pub html: String,
    /// Page title from document.title.
    pub title: Option<String>,
    /// Whether DOM was truncated due to size limit.
    pub dom_truncated: bool,
    /// Whether non-GET/HEAD subrequests were blocked.
    pub blocked_non_get: bool,
}

#[derive(Debug)]
struct InterceptState {
    blocked_non_get: AtomicBool,
    total_subresource_bytes: AtomicU64,
    budget_exhausted: AtomicBool,
    in_flight: AtomicUsize,
    main_frame_id: Mutex<Option<page::FrameId>>,
    main_request_count: AtomicUsize,
}

impl InterceptState {
    fn new() -> Self {
        Self {
            blocked_non_get: AtomicBool::new(false),
            total_subresource_bytes: AtomicU64::new(0),
            budget_exhausted: AtomicBool::new(false),
            in_flight: AtomicUsize::new(0),
            main_frame_id: Mutex::new(None),
            main_request_count: AtomicUsize::new(0),
        }
    }
}

fn normalize_blocked_types(blocked: &[String]) -> HashSet<String> {
    blocked
        .iter()
        .map(|s| s.trim().to_ascii_lowercase())
        .filter(|s| !s.is_empty())
        .collect()
}

fn resource_type_name(resource_type: &network::ResourceType) -> String {
    format!("{resource_type:?}").to_ascii_lowercase()
}

fn is_blocked_resource(resource_type: &network::ResourceType, blocked: &HashSet<String>) -> bool {
    let name = resource_type_name(resource_type);
    blocked.contains(&name)
}

fn is_document(resource_type: &network::ResourceType) -> bool {
    matches!(resource_type, network::ResourceType::Document)
}

fn is_websocket(resource_type: &network::ResourceType) -> bool {
    matches!(resource_type, network::ResourceType::WebSocket)
}

fn is_allowed_method(method: &str) -> bool {
    matches!(method, "GET" | "HEAD")
}

fn timeout_error(phase: TimeoutPhase, timeout: Duration) -> WebFetchError {
    WebFetchError::new(ErrorCode::Timeout, "browser timeout", true)
        .with_detail("phase", timeout_phase_label(phase))
        .with_detail("timeout_ms", timeout.as_millis().to_string())
}

fn timeout_phase_label(phase: TimeoutPhase) -> &'static str {
    match phase {
        TimeoutPhase::Dns => "dns",
        TimeoutPhase::Connect => "connect",
        TimeoutPhase::Tls => "tls",
        TimeoutPhase::Request => "request",
        TimeoutPhase::Response => "response",
        TimeoutPhase::Redirect => "redirect",
        TimeoutPhase::BrowserNavigation => "browser_navigation",
        TimeoutPhase::BrowserNetworkIdle => "browser_network_idle",
        TimeoutPhase::Robots => "robots",
    }
}

fn build_request_headers(request: &network::Request) -> Vec<(String, String)> {
    let mut headers = Vec::new();
    if let Some(obj) = request.headers.inner().as_object() {
        for (k, v) in obj {
            if let Some(value) = v.as_str() {
                let key = k.to_ascii_lowercase();
                if key == "accept" || key == "accept-encoding" || key == "referer" {
                    headers.push((k.clone(), value.to_string()));
                }
            }
        }
    }
    if !headers
        .iter()
        .any(|(k, _)| k.eq_ignore_ascii_case("accept"))
    {
        headers.push(("Accept".to_string(), "*/*".to_string()));
    }
    if !headers
        .iter()
        .any(|(k, _)| k.eq_ignore_ascii_case("accept-encoding"))
    {
        headers.push((
            "Accept-Encoding".to_string(),
            "gzip, deflate, br".to_string(),
        ));
    }
    headers
}

async fn spawn_network_tracker(
    page: &Page,
    state: Arc<InterceptState>,
) -> Result<(), WebFetchError> {
    let mut will_be_sent = page
        .event_listener::<network::EventRequestWillBeSent>()
        .await
        .map_err(|e| {
            WebFetchError::new(
                ErrorCode::BrowserCrashed,
                format!("failed to subscribe to request events: {e}"),
                true,
            )
        })?;
    let mut finished = page
        .event_listener::<network::EventLoadingFinished>()
        .await
        .map_err(|e| {
            WebFetchError::new(
                ErrorCode::BrowserCrashed,
                format!("failed to subscribe to loading finished events: {e}"),
                true,
            )
        })?;
    let mut failed = page
        .event_listener::<network::EventLoadingFailed>()
        .await
        .map_err(|e| {
            WebFetchError::new(
                ErrorCode::BrowserCrashed,
                format!("failed to subscribe to loading failed events: {e}"),
                true,
            )
        })?;

    let state_for_will = state.clone();
    tokio::spawn(async move {
        while (will_be_sent.next().await).is_some() {
            state_for_will.in_flight.fetch_add(1, Ordering::Relaxed);
        }
    });

    let state_for_finished = state.clone();
    tokio::spawn(async move {
        while (finished.next().await).is_some() {
            state_for_finished.in_flight.fetch_sub(1, Ordering::Relaxed);
        }
    });

    let state_for_failed = state.clone();
    tokio::spawn(async move {
        while (failed.next().await).is_some() {
            state_for_failed.in_flight.fetch_sub(1, Ordering::Relaxed);
        }
    });

    Ok(())
}

async fn spawn_request_handler(
    page: &Page,
    state: Arc<InterceptState>,
    config: ResolvedConfig,
    blocked_resource_types: HashSet<String>,
    fatal_tx: mpsc::UnboundedSender<WebFetchError>,
) -> Result<(), WebFetchError> {
    let mut paused = page
        .event_listener::<fetch::EventRequestPaused>()
        .await
        .map_err(|e| {
            WebFetchError::new(
                ErrorCode::BrowserCrashed,
                format!("failed to subscribe to requestPaused events: {e}"),
                true,
            )
        })?;
    let page = page.clone();

    tokio::spawn(async move {
        while let Some(event) = paused.next().await {
            if let Err(err) =
                handle_request(&page, &state, &config, &blocked_resource_types, &event).await
            {
                let _ = fatal_tx.send(err);
            }
        }
    });

    Ok(())
}

async fn handle_request(
    page: &Page,
    state: &InterceptState,
    config: &ResolvedConfig,
    blocked_resource_types: &HashSet<String>,
    event: &fetch::EventRequestPaused,
) -> Result<(), WebFetchError> {
    let request = &event.request;
    let resource_type = event.resource_type.clone();
    let browser_cfg = config.browser.as_enabled();
    let request_url = Url::parse(&request.url)
        .map_err(|_| WebFetchError::new(ErrorCode::InvalidUrl, "invalid request URL", false))?;

    // Determine main document frame
    let is_doc = is_document(&resource_type);
    let is_main = if is_doc {
        let mut guard = state.main_frame_id.lock().await;
        if let Some(frame_id) = guard.as_ref() {
            frame_id == &event.frame_id
        } else {
            *guard = Some(event.frame_id.clone());
            true
        }
    } else {
        false
    };

    if is_doc {
        let count = state.main_request_count.fetch_add(1, Ordering::Relaxed) + 1;
        if count > (config.max_redirects as usize + 1) && is_main {
            let _ = page
                .execute(fetch::FailRequestParams {
                    request_id: event.request_id.clone(),
                    error_reason: network::ErrorReason::BlockedByClient,
                })
                .await;
            return Err(WebFetchError::new(
                ErrorCode::RedirectLimit,
                "redirect limit exceeded",
                false,
            ));
        }
    }

    // Scheme enforcement
    match request_url.scheme() {
        "http" | "https" => {}
        _ => {
            fail_request(page, event).await;
            return Ok(());
        }
    }

    // Method restrictions
    if !is_allowed_method(&request.method) {
        state.blocked_non_get.store(true, Ordering::Relaxed);
        fail_request(page, event).await;
        return Ok(());
    }

    // Resource blocking
    if is_blocked_resource(&resource_type, blocked_resource_types) {
        fail_request(page, event).await;
        return Ok(());
    }

    // Subresource budget enforcement
    if !is_doc && !is_websocket(&resource_type) {
        if state.budget_exhausted.load(Ordering::Relaxed) {
            fail_request(page, event).await;
            return Ok(());
        }
        if state.total_subresource_bytes.load(Ordering::Relaxed)
            >= browser_cfg.max_subresource_bytes
        {
            state.budget_exhausted.store(true, Ordering::Relaxed);
            fail_request(page, event).await;
            return Ok(());
        }
    }

    // SSRF validation
    let resolved_ips = match http::validate_url(&request.url, &request_url, config).await {
        Ok(ips) => ips,
        Err(err) => {
            if is_main {
                fail_request(page, event).await;
                return Err(err);
            }
            fail_request(page, event).await;
            return Ok(());
        }
    };

    let headers = build_request_headers(request);
    let deadline = StdInstant::now() + browser_cfg.timeout;
    let method = if request.method.eq_ignore_ascii_case("HEAD") {
        reqwest::Method::HEAD
    } else {
        reqwest::Method::GET
    };

    let response = match fetch_with_redirects(
        &request_url,
        &resolved_ips,
        method,
        &headers,
        config,
        deadline,
    )
    .await
    {
        Ok(resp) => resp,
        Err(err) => {
            if is_main {
                fail_request(page, event).await;
                return Err(err);
            }
            fail_request(page, event).await;
            return Ok(());
        }
    };

    // Main document status handling
    if is_main {
        if response.status == 200 {
            // continue
        } else if matches!(response.status, 301 | 302 | 303 | 307 | 308) {
            // allow redirects
        } else if response.status == 204
            || response.status == 304
            || (100..200).contains(&response.status)
        {
            fail_request(page, event).await;
            return Err(
                WebFetchError::new(ErrorCode::Network, "unexpected status", true)
                    .with_detail("error", "unexpected_status")
                    .with_detail("status", response.status.to_string()),
            );
        } else if (400..500).contains(&response.status) {
            let retryable = matches!(response.status, 408 | 429);
            fail_request(page, event).await;
            return Err(WebFetchError::new(
                ErrorCode::Http4xx,
                format!("HTTP {}", response.status),
                retryable,
            )
            .with_detail("status", response.status.to_string())
            .with_detail("status_text", ""));
        } else if (500..600).contains(&response.status) {
            fail_request(page, event).await;
            return Err(WebFetchError::new(
                ErrorCode::Http5xx,
                format!("HTTP {}", response.status),
                true,
            )
            .with_detail("status", response.status.to_string())
            .with_detail("status_text", ""));
        }
    }

    // Budget accounting for subresources
    if !is_doc && !is_websocket(&resource_type) {
        let size = response.body.len() as u64;
        let total = state
            .total_subresource_bytes
            .fetch_add(size, Ordering::Relaxed)
            + size;
        if total >= browser_cfg.max_subresource_bytes {
            state.budget_exhausted.store(true, Ordering::Relaxed);
        }
    }

    fulfill_request(page, event, response).await;
    Ok(())
}

async fn fail_request(page: &Page, event: &fetch::EventRequestPaused) {
    let _ = page
        .execute(fetch::FailRequestParams {
            request_id: event.request_id.clone(),
            error_reason: network::ErrorReason::BlockedByClient,
        })
        .await;
}

async fn fulfill_request(
    page: &Page,
    event: &fetch::EventRequestPaused,
    response: SubrequestResponse,
) {
    let mut headers = response.headers;
    headers.retain(|(k, _)| !k.eq_ignore_ascii_case("content-encoding"));
    headers.retain(|(k, _)| !k.eq_ignore_ascii_case("content-length"));
    headers.push((
        "Content-Length".to_string(),
        response.body.len().to_string(),
    ));

    let response_headers = headers
        .into_iter()
        .map(|(name, value)| fetch::HeaderEntry { name, value })
        .collect::<Vec<_>>();

    let body = STANDARD.encode(&response.body);

    let _ = page
        .execute(fetch::FulfillRequestParams {
            request_id: event.request_id.clone(),
            response_code: i64::from(response.status),
            response_headers: Some(response_headers),
            binary_response_headers: None,
            body: Some(body.into()),
            response_phrase: None,
        })
        .await;
}

struct SubrequestResponse {
    status: u16,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

async fn fetch_with_redirects(
    url: &Url,
    resolved_ips: &[std::net::IpAddr],
    method: reqwest::Method,
    headers: &[(String, String)],
    config: &ResolvedConfig,
    deadline: StdInstant,
) -> Result<SubrequestResponse, WebFetchError> {
    let mut current_url = url.clone();
    let mut current_ips = resolved_ips.to_vec();
    let mut redirect_count = 0u32;

    loop {
        let response = http::send_with_pinning(
            &current_url,
            &current_ips,
            method.clone(),
            headers,
            config,
            deadline,
        )
        .await?;

        let status = response.status().as_u16();
        if matches!(status, 301 | 302 | 303 | 307 | 308) {
            redirect_count += 1;
            if redirect_count > config.max_redirects {
                return Err(WebFetchError::new(
                    ErrorCode::RedirectLimit,
                    "redirect limit exceeded",
                    false,
                ));
            }

            let location = response
                .headers()
                .get(reqwest::header::LOCATION)
                .and_then(|v| v.to_str().ok())
                .unwrap_or("");
            if location.is_empty() {
                return Err(WebFetchError::new(
                    ErrorCode::InvalidUrl,
                    "redirect missing Location header",
                    false,
                ));
            }

            let next_url = current_url.join(location).map_err(|_| {
                WebFetchError::new(
                    ErrorCode::InvalidUrl,
                    "redirect Location could not be resolved",
                    false,
                )
            })?;

            let raw_for_validation = if Url::parse(location).is_ok() {
                location
            } else {
                next_url.as_str()
            };
            current_ips = http::validate_url(raw_for_validation, &next_url, config).await?;
            current_url = next_url;
            continue;
        }

        let mut headers_out = Vec::new();
        for (name, value) in response.headers() {
            if let Ok(value) = value.to_str() {
                headers_out.push((name.to_string(), value.to_string()));
            }
        }

        let mut body = Vec::new();
        let mut stream = response.bytes_stream();
        let max_bytes = config.max_download_bytes as usize;
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|e| {
                WebFetchError::new(
                    ErrorCode::Network,
                    format!("response stream error: {e}"),
                    true,
                )
            })?;
            if body.len() + chunk.len() > max_bytes {
                return Err(WebFetchError::new(
                    ErrorCode::ResponseTooLarge,
                    "response exceeds size limit",
                    false,
                ));
            }
            body.extend_from_slice(&chunk);
        }

        return Ok(SubrequestResponse {
            status,
            headers: headers_out,
            body,
        });
    }
}

async fn wait_for_network_idle(
    state: Arc<InterceptState>,
    deadline: TokioInstant,
) -> Result<(), WebFetchError> {
    let idle_required = Duration::from_millis(500);
    let mut last_idle_start: Option<TokioInstant> = None;

    loop {
        if TokioInstant::now() > deadline {
            return Err(timeout_error(
                TimeoutPhase::BrowserNetworkIdle,
                deadline.saturating_duration_since(TokioInstant::now()),
            ));
        }

        let in_flight = state.in_flight.load(Ordering::Relaxed);
        if in_flight == 0 {
            if last_idle_start.is_none() {
                last_idle_start = Some(TokioInstant::now());
            }
            if let Some(start) = last_idle_start
                && TokioInstant::now().duration_since(start) >= idle_required
            {
                return Ok(());
            }
        } else {
            last_idle_start = None;
        }

        sleep(Duration::from_millis(50)).await;
    }
}

async fn extract_outer_html(page: &Page) -> Result<String, WebFetchError> {
    let result = page
        .evaluate("document.documentElement.outerHTML")
        .await
        .map_err(|e| {
            WebFetchError::new(
                ErrorCode::BrowserCrashed,
                format!("failed to evaluate outerHTML: {e}"),
                true,
            )
        })?;
    result.into_value::<String>().map_err(|_| {
        WebFetchError::new(ErrorCode::ExtractionFailed, "dom extraction failed", false)
    })
}

struct TempProfileDir {
    path: PathBuf,
}

impl TempProfileDir {
    fn new() -> Result<Self, WebFetchError> {
        let base = std::env::temp_dir();
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let pid = std::process::id();

        for attempt in 0..10 {
            let mut path = base.clone();
            path.push(format!("forge-webfetch-{pid}-{timestamp}-{attempt}"));
            if fs::create_dir_all(&path).is_ok() {
                return Ok(Self { path });
            }
        }

        Err(WebFetchError::new(
            ErrorCode::Internal,
            "failed to create temporary browser profile directory",
            false,
        ))
    }
}

impl Drop for TempProfileDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

fn truncate_utf8(value: &str, max_bytes: usize) -> String {
    if value.len() <= max_bytes {
        return value.to_string();
    }
    let mut end = max_bytes.min(value.len());
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    value[..end].to_string()
}

fn extract_title(html: &str) -> Option<String> {
    let lower = html.to_ascii_lowercase();
    let start = lower.find("<title")?;
    let after = &lower[start..];
    let tag_end = after.find('>')? + start;
    let close = lower[tag_end..].find("</title>")? + tag_end;
    let raw = &html[tag_end + 1..close];
    let title = raw.trim();
    if title.is_empty() {
        None
    } else {
        Some(title.to_string())
    }
}

fn find_chromium() -> Option<PathBuf> {
    if let Some(path) = find_on_path(&chromium_candidates()) {
        return Some(path);
    }

    platform_chromium_paths()
        .into_iter()
        .find(|path| path.exists())
}

fn find_on_path(candidates: &[&str]) -> Option<PathBuf> {
    let path_var = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path_var) {
        for candidate in candidates {
            let full = dir.join(candidate);
            if full.exists() {
                return Some(full);
            }
        }
    }
    None
}

#[cfg(target_os = "windows")]
fn chromium_candidates() -> Vec<&'static str> {
    vec!["chromium.exe", "chrome.exe"]
}

#[cfg(target_os = "macos")]
fn chromium_candidates() -> Vec<&'static str> {
    vec!["chromium", "google-chrome", "chrome"]
}

#[cfg(all(unix, not(target_os = "macos")))]
fn chromium_candidates() -> Vec<&'static str> {
    vec![
        "chromium",
        "chromium-browser",
        "google-chrome",
        "google-chrome-stable",
    ]
}

#[cfg(target_os = "windows")]
fn platform_chromium_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    let program_files = std::env::var_os("ProgramFiles");
    let program_files_x86 = std::env::var_os("ProgramFiles(x86)");
    let local_app_data = std::env::var_os("LOCALAPPDATA");

    if let Some(base) = program_files.as_ref() {
        let base = PathBuf::from(base);
        paths.push(base.join("Google/Chrome/Application/chrome.exe"));
        paths.push(base.join("Chromium/Application/chrome.exe"));
    }
    if let Some(base) = program_files_x86.as_ref() {
        let base = PathBuf::from(base);
        paths.push(base.join("Google/Chrome/Application/chrome.exe"));
        paths.push(base.join("Chromium/Application/chrome.exe"));
    }
    if let Some(base) = local_app_data {
        paths.push(PathBuf::from(base).join("Chromium/Application/chrome.exe"));
    }

    paths
}

#[cfg(target_os = "macos")]
fn platform_chromium_paths() -> Vec<PathBuf> {
    vec![
        PathBuf::from("/Applications/Chromium.app/Contents/MacOS/Chromium"),
        PathBuf::from("/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"),
        PathBuf::from("/Applications/Google Chrome Canary.app/Contents/MacOS/Google Chrome Canary"),
        PathBuf::from("/usr/local/bin/chromium"),
        PathBuf::from("/opt/homebrew/bin/chromium"),
    ]
}

#[cfg(all(unix, not(target_os = "macos")))]
fn platform_chromium_paths() -> Vec<PathBuf> {
    vec![
        PathBuf::from("/usr/bin/chromium"),
        PathBuf::from("/usr/bin/chromium-browser"),
        PathBuf::from("/usr/bin/google-chrome"),
        PathBuf::from("/usr/bin/google-chrome-stable"),
        PathBuf::from("/snap/bin/chromium"),
    ]
}

trait BrowserPolicyExt {
    fn as_enabled(&self) -> &crate::resolved::ResolvedBrowserConfig;
}

impl BrowserPolicyExt for BrowserPolicy {
    fn as_enabled(&self) -> &crate::resolved::ResolvedBrowserConfig {
        match self {
            BrowserPolicy::Enabled(cfg) => cfg,
            BrowserPolicy::Disabled => unreachable!("browser policy disabled"),
        }
    }
}
