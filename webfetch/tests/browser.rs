//! Browser-mode integration tests for WebFetch.
//!
//! These tests are skipped unless FORGE_TEST_CHROMIUM_PATH is set.

use forge_webfetch::{
    BrowserConfig, Note, RenderingMethod, RobotsConfig, SecurityConfig, WebFetchConfig,
    WebFetchInput,
};
use std::env;
use std::path::PathBuf;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn base_browser_config() -> WebFetchConfig {
    WebFetchConfig {
        enabled: true,
        user_agent: Some("forge-test/1.0".to_string()),
        timeout_seconds: Some(10),
        max_redirects: Some(5),
        max_download_bytes: Some(1_000_000),
        max_cache_entries: Some(0),
        robots_cache_entries: Some(0),
        robots_cache_ttl_hours: Some(1),
        robots: Some(RobotsConfig {
            user_agent_token: Some("forge-test".to_string()),
            fail_open: true,
        }),
        security: Some(SecurityConfig {
            allow_insecure_overrides: true,
            ..Default::default()
        }),
        ..Default::default()
    }
}

fn config_with_browser(chromium_path: PathBuf, enabled: bool) -> WebFetchConfig {
    let mut config = base_browser_config();
    config.browser = Some(BrowserConfig {
        enabled,
        chromium_path: Some(chromium_path),
        ..Default::default()
    });
    config
}

async fn setup_server(html: &str) -> MockServer {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/robots.txt"))
        .respond_with(ResponseTemplate::new(200).set_body_string("User-agent: *\nAllow: /"))
        .mount(&server)
        .await;

    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("Content-Type", "text/html; charset=utf-8")
                .set_body_string(html),
        )
        .mount(&server)
        .await;

    server
}

#[tokio::test]
async fn test_force_browser_unavailable_falls_back_to_http() {
    let html = "<html><body><main><h1>Fallback</h1><p>This page has enough text content to pass extraction.</p></main></body></html>";
    let server = setup_server(html).await;
    let input = WebFetchInput::new(server.uri())
        .expect("valid URL")
        .with_force_browser(true);

    let mut config = base_browser_config();
    config.browser = Some(BrowserConfig {
        enabled: false,
        ..Default::default()
    });

    let output = forge_webfetch::fetch(input, &config)
        .await
        .expect("fetch should succeed via HTTP fallback");

    assert_eq!(output.rendering_method, RenderingMethod::Http);
    assert!(output.notes.contains(&Note::BrowserUnavailableUsedHttp));
}

#[tokio::test]
async fn test_force_browser_renders_when_available() {
    let chromium_path = match env::var("FORGE_TEST_CHROMIUM_PATH") {
        Ok(path) => PathBuf::from(path),
        Err(_) => {
            eprintln!("FORGE_TEST_CHROMIUM_PATH not set; skipping browser test");
            return;
        }
    };

    let html = "<html><body><main><h1>Browser Render</h1><p>This page has enough text content to pass extraction.</p></main></body></html>";
    let server = setup_server(html).await;
    let input = WebFetchInput::new(server.uri())
        .expect("valid URL")
        .with_force_browser(true);

    let config = config_with_browser(chromium_path, true);
    let output = forge_webfetch::fetch(input, &config)
        .await
        .expect("browser fetch should succeed");

    assert_eq!(output.rendering_method, RenderingMethod::Browser);
    assert!(!output.notes.contains(&Note::BrowserUnavailableUsedHttp));
}
