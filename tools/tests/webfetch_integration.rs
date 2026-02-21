//! Integration tests for the WebFetch tool.
//!
//! These tests exercise the full fetch pipeline: URL validation → robots.txt →
//! HTTP fetch → extraction → chunking → caching.

use forge_tools::webfetch::{
    CachePreference, ErrorCode, HttpConfig, Note, OutputCompleteness, RobotsConfig, SecurityConfig,
    WebFetchConfig, WebFetchInput, fetch,
};
use std::env;
use std::path::Path;
use std::sync::Once;
use std::time::Duration;
use tempfile::TempDir;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::task::JoinHandle;
use tokio::time::sleep;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const GZIP_BOMB_PAYLOAD: &[u8] = &[
    0x1f, 0x8b, 0x08, 0x00, 0x00, 0x00, 0x00, 0x00, 0x02, 0xff, 0xed, 0xc1, 0x81, 0x00, 0x00, 0x00,
    0x00, 0x80, 0x20, 0xb6, 0xfd, 0xa5, 0x16, 0xa9, 0x0a, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x6a, 0x80, 0x06, 0x9b, 0xa0, 0x00, 0x00, 0x01, 0x00,
];

fn enable_insecure_webfetch_opt_in_for_tests() {
    static INIT: Once = Once::new();
    INIT.call_once(|| {
        // SAFETY: integration tests require loopback fetch; set once and never mutate.
        unsafe {
            env::set_var("FORGE_WEBFETCH_ALLOW_INSECURE_OVERRIDES", "1");
        }
    });
}

fn test_config() -> WebFetchConfig {
    enable_insecure_webfetch_opt_in_for_tests();
    WebFetchConfig {
        enabled: true,
        user_agent: Some("forge-test/1.0".to_string()),
        timeout_seconds: Some(5),
        max_redirects: Some(5),
        max_download_bytes: Some(1_000_000),
        max_cache_entries: Some(0), // Disable cache for most tests
        robots_cache_entries: Some(0),
        robots_cache_ttl_hours: Some(1),
        robots: Some(RobotsConfig {
            user_agent_token: Some("forge-test".to_string()),
            fail_open: true,
        }),
        security: Some(SecurityConfig {
            allow_insecure_overrides: true, // Allow loopback for wiremock
            ..Default::default()
        }),
        http: Some(HttpConfig::default()),
        ..Default::default()
    }
}

fn test_config_secure() -> WebFetchConfig {
    let mut config = test_config();
    if let Some(security) = config.security.as_mut() {
        security.allow_insecure_overrides = false;
    }
    config
}

fn test_config_with_cache(cache_dir: &Path) -> WebFetchConfig {
    let mut config = test_config();
    config.max_cache_entries = Some(100);
    config.cache_dir = Some(cache_dir.to_path_buf());
    config.cache_ttl_days = Some(1);
    config.max_cache_bytes = Some(10_000_000);
    config
}

fn simple_html(title: &str, body: &str) -> String {
    let filler = "Additional text ensures extraction passes minimum length checks for tests.";
    format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="utf-8">
    <title>{title}</title>
</head>
<body>
    <main>
        <h1>{title}</h1>
        <p>{body} {filler}</p>
    </main>
</body>
</html>"#
    )
}

fn multi_section_html() -> String {
    let extra = "This filler sentence increases the token count for chunking tests. ".repeat(12);
    format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="utf-8">
    <title>Multi-Section Document</title>
</head>
<body>
    <main>
        <h1>Main Title</h1>
        <p>Introduction paragraph with some content.</p>

        <h2>Section One</h2>
        <p>This is the first section with detailed content. It contains multiple sentences to ensure we have enough text for chunking tests. The goal is to have substantial content that will be processed by the extraction pipeline.</p>

        <h2>Section Two</h2>
        <p>Second section content goes here. More text to fill out the document and test heading tracking across chunks. Additional sentences extend the content and help ensure multiple chunks are produced when the token limit is small.</p>

        <h3>Subsection</h3>
        <p>A subsection with its own content.</p>

        <h2>Section Three</h2>
        <p>Final section with closing remarks.</p>
        <p>{extra}</p>
    </main>
</body>
</html>"#
    )
}

async fn setup_mock_server_with_robots(html: &str) -> MockServer {
    let server = MockServer::start().await;

    // robots.txt allowing everything
    Mock::given(method("GET"))
        .and(path("/robots.txt"))
        .respond_with(ResponseTemplate::new(200).set_body_string("User-agent: *\nAllow: /"))
        .mount(&server)
        .await;

    // Main page
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

async fn setup_robots_body_stall_server(delay: Duration) -> (String, JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind local listener");
    let address = listener.local_addr().expect("listener local_addr");

    let handle = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.expect("accept connection");

        let mut request = [0u8; 2048];
        let _ = socket.read(&mut request).await;

        let first = b"User-agent: *\n";
        let second = b"Allow: /\n";
        let content_length = first.len() + second.len();
        let header = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: {content_length}\r\nConnection: close\r\n\r\n"
        );
        socket
            .write_all(header.as_bytes())
            .await
            .expect("write headers");
        socket
            .write_all(first)
            .await
            .expect("write first robots bytes");
        socket.flush().await.expect("flush first robots bytes");
        sleep(delay).await;
        socket
            .write_all(second)
            .await
            .expect("write second robots bytes");
        socket.flush().await.expect("flush second robots bytes");
    });

    (format!("http://{address}"), handle)
}

#[tokio::test]
async fn test_basic_fetch_success() {
    let html = simple_html("Test Page", "Hello, World!");
    let server = setup_mock_server_with_robots(&html).await;

    let input = WebFetchInput::new(server.uri()).expect("valid URL");
    let config = test_config();

    let output = fetch(input, &config).await.expect("fetch should succeed");

    assert_eq!(output.title, Some("Test Page".to_string()));
    assert_eq!(output.language, Some("en".to_string()));
    assert!(matches!(output.completeness, OutputCompleteness::Complete));
    assert!(!output.chunks.is_empty());

    let all_text: String = output.chunks.iter().map(|c| c.text.as_str()).collect();
    assert!(all_text.contains("Hello, World!"));
}

#[tokio::test]
async fn test_fetch_with_path() {
    let server = MockServer::start().await;

    // robots.txt
    Mock::given(method("GET"))
        .and(path("/robots.txt"))
        .respond_with(ResponseTemplate::new(200).set_body_string("User-agent: *\nAllow: /"))
        .mount(&server)
        .await;

    // Specific page
    Mock::given(method("GET"))
        .and(path("/docs/guide"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("Content-Type", "text/html; charset=utf-8")
                .set_body_string(simple_html("Guide", "Documentation content")),
        )
        .mount(&server)
        .await;

    let url = format!("{}/docs/guide", server.uri());
    let input = WebFetchInput::new(&url).expect("valid URL");
    let config = test_config();

    let output = fetch(input, &config).await.expect("fetch should succeed");

    assert_eq!(output.title, Some("Guide".to_string()));
    assert!(output.final_url.contains("/docs/guide"));
}

#[tokio::test]
async fn test_fetch_preserves_requested_url() {
    let html = simple_html("Test", "Content");
    let server = setup_mock_server_with_robots(&html).await;

    let requested = server.uri();
    let input = WebFetchInput::new(&requested).expect("valid URL");
    let config = test_config();

    let output = fetch(input, &config).await.expect("fetch should succeed");

    assert!(output.requested_url.starts_with(&requested));
}

#[tokio::test]
async fn test_robots_disallow_blocks_fetch() {
    let server = MockServer::start().await;

    // robots.txt disallowing /private
    Mock::given(method("GET"))
        .and(path("/robots.txt"))
        .respond_with(
            ResponseTemplate::new(200).set_body_string("User-agent: *\nDisallow: /private/"),
        )
        .mount(&server)
        .await;

    // The page exists but should be blocked
    Mock::given(method("GET"))
        .and(path("/private/secret"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("Content-Type", "text/html")
                .set_body_string(simple_html("Secret", "Hidden")),
        )
        .mount(&server)
        .await;

    let url = format!("{}/private/secret", server.uri());
    let input = WebFetchInput::new(&url).expect("valid URL");
    let config = test_config();

    let result = fetch(input, &config).await;

    assert!(result.is_err());
    let err = result.unwrap_err();
    assert_eq!(err.code, ErrorCode::RobotsDisallowed);
}

#[tokio::test]
async fn test_robots_404_allows_fetch() {
    let server = MockServer::start().await;

    // robots.txt returns 404 (should allow all)
    Mock::given(method("GET"))
        .and(path("/robots.txt"))
        .respond_with(ResponseTemplate::new(404))
        .mount(&server)
        .await;

    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("Content-Type", "text/html")
                .set_body_string(simple_html("Public", "Content")),
        )
        .mount(&server)
        .await;

    let input = WebFetchInput::new(server.uri()).expect("valid URL");
    let config = test_config();

    let output = fetch(input, &config).await.expect("fetch should succeed");
    assert_eq!(output.title, Some("Public".to_string()));
}

#[tokio::test]
async fn test_robots_user_agent_specific_rules() {
    let server = MockServer::start().await;

    // robots.txt with user-agent specific rules
    Mock::given(method("GET"))
        .and(path("/robots.txt"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string("User-agent: forge-test\nAllow: /\n\nUser-agent: *\nDisallow: /"),
        )
        .mount(&server)
        .await;

    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("Content-Type", "text/html")
                .set_body_string(simple_html("Allowed", "Content")),
        )
        .mount(&server)
        .await;

    let input = WebFetchInput::new(server.uri()).expect("valid URL");
    let config = test_config(); // Uses forge-test UA

    let output = fetch(input, &config).await.expect("fetch should succeed");
    assert_eq!(output.title, Some("Allowed".to_string()));
}

#[tokio::test]
async fn test_extraction_removes_boilerplate() {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/robots.txt"))
        .respond_with(ResponseTemplate::new(200).set_body_string("User-agent: *\nAllow: /"))
        .mount(&server)
        .await;

    // HTML with boilerplate elements
    let html = r"<!DOCTYPE html>
<html>
<head><title>Clean Page</title></head>
<body>
    <nav>Navigation links</nav>
    <header>Site Header</header>
    <main>
        <h1>Main Content</h1>
        <p>This is the actual content we want to extract.</p>
    </main>
    <footer>Footer content</footer>
    <script>alert('js');</script>
</body>
</html>";

    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("Content-Type", "text/html")
                .set_body_string(html),
        )
        .mount(&server)
        .await;

    let input = WebFetchInput::new(server.uri()).expect("valid URL");
    let config = test_config();

    let output = fetch(input, &config).await.expect("fetch should succeed");

    let all_text: String = output.chunks.iter().map(|c| c.text.as_str()).collect();

    // Main content should be present
    assert!(all_text.contains("Main Content"));
    assert!(all_text.contains("actual content"));

    // Boilerplate should be removed
    assert!(!all_text.contains("Navigation links"));
    assert!(!all_text.contains("Site Header"));
    assert!(!all_text.contains("Footer content"));
    assert!(!all_text.contains("alert"));
}

#[tokio::test]
async fn test_extraction_converts_links() {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/robots.txt"))
        .respond_with(ResponseTemplate::new(200).set_body_string("User-agent: *\nAllow: /"))
        .mount(&server)
        .await;

    let html = format!(
        r#"<!DOCTYPE html>
<html>
<head><title>Links</title></head>
<body>
    <main>
        <p>Visit <a href="/page">relative link</a> or <a href="{}/absolute">absolute link</a>. Additional text ensures extraction passes minimum length checks for tests.</p>
    </main>
</body>
</html>"#,
        server.uri()
    );

    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("Content-Type", "text/html")
                .set_body_string(html),
        )
        .mount(&server)
        .await;

    let input = WebFetchInput::new(server.uri()).expect("valid URL");
    let config = test_config();

    let output = fetch(input, &config).await.expect("fetch should succeed");
    let all_text: String = output.chunks.iter().map(|c| c.text.as_str()).collect();

    // Links should be converted to markdown format with absolute URLs
    assert!(all_text.contains("[relative link]"));
    assert!(all_text.contains("/page"));
}

#[tokio::test]
async fn test_chunking_respects_max_tokens() {
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
                .insert_header("Content-Type", "text/html")
                .set_body_string(multi_section_html()),
        )
        .mount(&server)
        .await;

    let input = WebFetchInput::new(server.uri())
        .expect("valid URL")
        .with_max_chunk_tokens(128)
        .expect("valid max_chunk_tokens"); // Small chunks

    let config = test_config();

    let output = fetch(input, &config).await.expect("fetch should succeed");

    // Should produce multiple chunks with small token limit
    assert!(
        output.chunks.len() > 1,
        "Expected multiple chunks with small token limit"
    );

    // Each chunk should respect the limit
    for chunk in &output.chunks {
        assert!(
            chunk.token_count <= 128,
            "Chunk exceeds token limit: {} > 128",
            chunk.token_count
        );
    }
}

#[tokio::test]
async fn test_chunking_tracks_headings() {
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
                .insert_header("Content-Type", "text/html")
                .set_body_string(multi_section_html()),
        )
        .mount(&server)
        .await;

    let input = WebFetchInput::new(server.uri())
        .expect("valid URL")
        .with_max_chunk_tokens(128)
        .expect("valid max_chunk_tokens"); // Very small to force multiple chunks

    let config = test_config();

    let output = fetch(input, &config).await.expect("fetch should succeed");

    // Should have chunks with heading context
    let chunks_with_headings: Vec<_> = output
        .chunks
        .iter()
        .filter(|c| !c.heading.is_empty())
        .collect();
    assert!(
        !chunks_with_headings.is_empty(),
        "Expected some chunks to have heading context"
    );
}

#[tokio::test]
async fn test_cache_hit_returns_cached_content() {
    let cache_dir = TempDir::new().expect("create temp dir");
    let html = simple_html("Cached Page", "Cached content");
    let server = setup_mock_server_with_robots(&html).await;

    let config = test_config_with_cache(cache_dir.path());
    let url = server.uri();

    // First fetch - should populate cache
    let input1 = WebFetchInput::new(&url).expect("valid URL");
    let output1 = fetch(input1, &config).await.expect("first fetch");

    assert!(
        !output1.notes.contains(&Note::CacheHit),
        "First fetch should not be cache hit"
    );

    // Second fetch - should be cache hit
    let input2 = WebFetchInput::new(&url).expect("valid URL");
    let output2 = fetch(input2, &config).await.expect("second fetch");

    assert!(
        output2.notes.contains(&Note::CacheHit),
        "Second fetch should be cache hit"
    );
    assert_eq!(output2.title, output1.title);
    assert_eq!(output2.final_url, output1.final_url);
}

#[tokio::test]
async fn test_no_cache_bypasses_cache() {
    let cache_dir = TempDir::new().expect("create temp dir");
    let html = simple_html("Page", "Content");
    let server = setup_mock_server_with_robots(&html).await;

    let config = test_config_with_cache(cache_dir.path());
    let url = server.uri();

    // First fetch to populate cache
    let input1 = WebFetchInput::new(&url).expect("valid URL");
    let _ = fetch(input1, &config).await.expect("first fetch");

    // Second fetch with bypass_cache preference - should bypass
    let input2 = WebFetchInput::new(&url)
        .expect("valid URL")
        .with_cache_preference(CachePreference::BypassCache);
    let output2 = fetch(input2, &config).await.expect("second fetch");

    assert!(
        !output2.notes.contains(&Note::CacheHit),
        "bypass_cache fetch should bypass cache"
    );
}

#[tokio::test]
async fn test_cache_rechunks_with_different_token_limit() {
    let cache_dir = TempDir::new().expect("create temp dir");
    let html = multi_section_html();
    let server = setup_mock_server_with_robots(&html).await;

    let config = test_config_with_cache(cache_dir.path());
    let url = server.uri();

    // First fetch with default tokens
    let input1 = WebFetchInput::new(&url)
        .expect("valid URL")
        .with_max_chunk_tokens(2000)
        .expect("valid max_chunk_tokens");
    let output1 = fetch(input1, &config).await.expect("first fetch");
    let chunk_count1 = output1.chunks.len();

    // Second fetch with smaller token limit - should re-chunk
    let input2 = WebFetchInput::new(&url)
        .expect("valid URL")
        .with_max_chunk_tokens(128)
        .expect("valid max_chunk_tokens");
    let output2 = fetch(input2, &config).await.expect("second fetch");

    assert!(output2.notes.contains(&Note::CacheHit));
    assert!(
        output2.chunks.len() > chunk_count1,
        "Smaller token limit should produce more chunks"
    );
}

#[tokio::test]
async fn test_invalid_url_rejected() {
    let result = WebFetchInput::new("not-a-url");
    assert!(result.is_err());
}

#[tokio::test]
async fn test_non_http_scheme_rejected() {
    let input = WebFetchInput::new("ftp://example.com/file").expect("valid URL");
    let config = test_config_secure();
    let result = fetch(input, &config).await;
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidScheme);
}

#[tokio::test]
async fn test_ssrf_localhost_blocked() {
    // Attempt to fetch localhost should be blocked
    let input = WebFetchInput::new("http://127.0.0.1/").expect("valid URL");
    let config = test_config_secure();

    let result = fetch(input, &config).await;

    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(matches!(
        err.code,
        ErrorCode::PortBlocked | ErrorCode::SsrfBlocked
    ));
}

#[tokio::test]
async fn test_ssrf_private_ip_blocked() {
    // Private IP ranges should be blocked
    let input = WebFetchInput::new("http://192.168.1.1/").expect("valid URL");
    let config = test_config_secure();

    let result = fetch(input, &config).await;

    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(matches!(
        err.code,
        ErrorCode::PortBlocked | ErrorCode::SsrfBlocked
    ));
}

#[tokio::test]
async fn test_insecure_overrides_still_block_private_ips() {
    let input = WebFetchInput::new("http://10.0.0.10/").expect("valid URL");
    let config = test_config(); // allow_insecure_overrides = true

    let result = fetch(input, &config).await;

    assert!(result.is_err());
    let err = result.unwrap_err();
    assert_eq!(err.code, ErrorCode::SsrfBlocked);
}

#[tokio::test]
async fn test_insecure_overrides_do_not_open_non_loopback_non_default_ports() {
    let input = WebFetchInput::new("http://93.184.216.34:8080/").expect("valid URL");
    let config = test_config(); // allow_insecure_overrides = true

    let result = fetch(input, &config).await;

    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(matches!(
        err.code,
        ErrorCode::PortBlocked | ErrorCode::SsrfBlocked
    ));
}

#[tokio::test]
async fn test_http_404_error() {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/robots.txt"))
        .respond_with(ResponseTemplate::new(200).set_body_string("User-agent: *\nAllow: /"))
        .mount(&server)
        .await;

    Mock::given(method("GET"))
        .and(path("/missing"))
        .respond_with(ResponseTemplate::new(404))
        .mount(&server)
        .await;

    let url = format!("{}/missing", server.uri());
    let input = WebFetchInput::new(&url).expect("valid URL");
    let config = test_config();

    let result = fetch(input, &config).await;

    assert!(result.is_err());
    let err = result.unwrap_err();
    assert_eq!(err.code, ErrorCode::Http4xx);
}

#[tokio::test]
async fn test_http_500_error() {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/robots.txt"))
        .respond_with(ResponseTemplate::new(200).set_body_string("User-agent: *\nAllow: /"))
        .mount(&server)
        .await;

    Mock::given(method("GET"))
        .and(path("/error"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&server)
        .await;

    let url = format!("{}/error", server.uri());
    let input = WebFetchInput::new(&url).expect("valid URL");
    let config = test_config();

    let result = fetch(input, &config).await;

    assert!(result.is_err());
    let err = result.unwrap_err();
    assert_eq!(err.code, ErrorCode::Http5xx);
}

#[tokio::test]
async fn test_non_html_content_type_rejected() {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/robots.txt"))
        .respond_with(ResponseTemplate::new(200).set_body_string("User-agent: *\nAllow: /"))
        .mount(&server)
        .await;

    Mock::given(method("GET"))
        .and(path("/data.json"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "application/json")
                .set_body_bytes(br#"{"key": "value"}"#),
        )
        .mount(&server)
        .await;

    let url = format!("{}/data.json", server.uri());
    let input = WebFetchInput::new(&url).expect("valid URL");
    let config = test_config();

    let result = fetch(input, &config).await;

    assert!(result.is_err());
    let err = result.unwrap_err();
    assert_eq!(err.code, ErrorCode::UnsupportedContentType);
}

#[tokio::test]
async fn test_compressed_response_over_limit_is_rejected() {
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
                .set_body_raw(GZIP_BOMB_PAYLOAD.to_vec(), "text/plain; charset=utf-8")
                .insert_header("Content-Encoding", "gzip"),
        )
        .mount(&server)
        .await;

    let mut config = test_config();
    config.max_download_bytes = Some(1024);

    let input = WebFetchInput::new(server.uri()).expect("valid URL");
    let result = fetch(input, &config).await;

    assert!(result.is_err());
    let err = result.unwrap_err();
    assert_eq!(err.code, ErrorCode::ResponseTooLarge);
}

#[tokio::test]
async fn test_robots_body_stream_timeout_returns_unavailable() {
    let (base_url, server_task) = setup_robots_body_stall_server(Duration::from_millis(1500)).await;

    let mut config = test_config();
    config.timeout_seconds = Some(1);
    if let Some(robots) = config.robots.as_mut() {
        robots.fail_open = false;
    }

    let input = WebFetchInput::new(&base_url).expect("valid URL");
    let result = fetch(input, &config).await;
    server_task.abort();

    assert!(result.is_err());
    let err = result.unwrap_err();
    assert_eq!(err.code, ErrorCode::RobotsUnavailable);
}

#[tokio::test]
async fn test_redirect_followed() {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/robots.txt"))
        .respond_with(ResponseTemplate::new(200).set_body_string("User-agent: *\nAllow: /"))
        .mount(&server)
        .await;

    Mock::given(method("GET"))
        .and(path("/old"))
        .respond_with(ResponseTemplate::new(301).insert_header("Location", "/new"))
        .mount(&server)
        .await;

    Mock::given(method("GET"))
        .and(path("/new"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("Content-Type", "text/html")
                .set_body_string(simple_html("New Page", "Redirected content")),
        )
        .mount(&server)
        .await;

    let url = format!("{}/old", server.uri());
    let input = WebFetchInput::new(&url).expect("valid URL");
    let config = test_config();

    let output = fetch(input, &config).await.expect("fetch should succeed");

    assert!(
        output.requested_url.contains("/old"),
        "requested_url should be original"
    );
    assert!(
        output.final_url.contains("/new"),
        "final_url should be redirect target"
    );
    assert_eq!(output.title, Some("New Page".to_string()));
}

#[tokio::test]
async fn test_redirect_target_robots_disallowed() {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/robots.txt"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string("User-agent: *\nAllow: /start\nDisallow: /blocked"),
        )
        .mount(&server)
        .await;

    Mock::given(method("GET"))
        .and(path("/start"))
        .respond_with(ResponseTemplate::new(301).insert_header("Location", "/blocked"))
        .mount(&server)
        .await;

    Mock::given(method("GET"))
        .and(path("/blocked"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("Content-Type", "text/html")
                .set_body_string(simple_html("Blocked", "This should not be fetched")),
        )
        .mount(&server)
        .await;

    let url = format!("{}/start", server.uri());
    let input = WebFetchInput::new(&url).expect("valid URL");
    let config = test_config();
    let err = fetch(input, &config)
        .await
        .expect_err("redirect target should be blocked by robots");
    assert_eq!(err.code, ErrorCode::RobotsDisallowed);
}

#[tokio::test]
async fn test_output_has_fetched_at_timestamp() {
    let html = simple_html("Test", "Content");
    let server = setup_mock_server_with_robots(&html).await;

    let input = WebFetchInput::new(server.uri()).expect("valid URL");
    let config = test_config();

    let output = fetch(input, &config).await.expect("fetch should succeed");

    // fetched_at should be a valid RFC3339 timestamp
    assert!(!output.fetched_at.is_empty());
    assert!(output.fetched_at.contains('T')); // RFC3339 format
    assert!(output.fetched_at.contains('Z') || output.fetched_at.contains('+')); // Timezone
}

#[tokio::test]
async fn test_chunk_has_token_count() {
    let html = simple_html("Test", "Some content for token counting");
    let server = setup_mock_server_with_robots(&html).await;

    let input = WebFetchInput::new(server.uri()).expect("valid URL");
    let config = test_config();

    let output = fetch(input, &config).await.expect("fetch should succeed");

    for chunk in &output.chunks {
        assert!(
            chunk.token_count > 0,
            "Each chunk should have positive token count"
        );
    }
}

#[tokio::test]
async fn test_url_fragment_removed_from_final_url() {
    let html = simple_html("Test", "Content");
    let server = setup_mock_server_with_robots(&html).await;

    let url_with_fragment = format!("{}#section", server.uri());
    let input = WebFetchInput::new(&url_with_fragment).expect("valid URL");
    let config = test_config();

    let output = fetch(input, &config).await.expect("fetch should succeed");

    // Fragment should be removed from final_url
    assert!(
        !output.final_url.contains('#'),
        "Fragment should be removed from final_url"
    );
}

#[tokio::test]
async fn test_http_upgraded_to_https() {
    // With secure config (allow_insecure_overrides = false), an http:// URL
    // should be upgraded to https://. We verify via a localhost URL which gets
    // SSRF-blocked — the scheme was rewritten before validation runs.
    let input = WebFetchInput::new("http://127.0.0.1/").expect("valid URL");
    let config = test_config_secure();

    let result = fetch(input, &config).await;

    // Should be SSRF blocked (upgrade happened, then SSRF check ran)
    assert!(result.is_err());
    assert_eq!(result.unwrap_err().code, ErrorCode::SsrfBlocked);
}

#[tokio::test]
async fn test_http_not_upgraded_when_insecure_overrides() {
    // With insecure overrides enabled, http:// URLs should NOT be upgraded.
    let html = simple_html("Test", "Content");
    let server = setup_mock_server_with_robots(&html).await;

    let input = WebFetchInput::new(server.uri()).expect("valid URL");
    let config = test_config(); // allow_insecure_overrides = true

    let output = fetch(input, &config).await.expect("fetch should succeed");

    // No upgrade note should appear
    assert!(
        !output.notes.contains(&Note::HttpUpgradedToHttps),
        "http should not be upgraded when insecure overrides are enabled"
    );
}
