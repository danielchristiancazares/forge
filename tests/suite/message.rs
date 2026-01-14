//! Message type tests

use forge_types::{Message, NonEmptyString, Provider};

#[test]
fn non_empty_string_rejects_empty() {
    assert!(NonEmptyString::new("").is_err());
    assert!(NonEmptyString::new("   ").is_err());
    assert!(NonEmptyString::new("\n\t").is_err());
}

#[test]
fn non_empty_string_accepts_content() {
    let s = NonEmptyString::new("hello").unwrap();
    assert_eq!(s.as_str(), "hello");
}

#[test]
fn non_empty_string_preserves_whitespace() {
    // Leading/trailing whitespace in content is preserved
    let s = NonEmptyString::new("  hello  ").unwrap();
    assert_eq!(s.as_str(), "  hello  ");
}

#[test]
fn non_empty_string_append() {
    let s = NonEmptyString::new("hello").unwrap();
    let s = s.append(" world");
    assert_eq!(s.as_str(), "hello world");
}

#[test]
fn non_empty_string_into_inner() {
    let s = NonEmptyString::new("test").unwrap();
    let inner: String = s.into_inner();
    assert_eq!(inner, "test");
}

#[test]
fn message_role_str() {
    let content = NonEmptyString::new("test").unwrap();
    let model = Provider::Claude.default_model();

    let system = Message::system(content.clone());
    let user = Message::user(content.clone());
    let assistant = Message::assistant(model, content);

    assert_eq!(system.role_str(), "system");
    assert_eq!(user.role_str(), "user");
    assert_eq!(assistant.role_str(), "assistant");
}

#[test]
fn message_content_access() {
    let content = NonEmptyString::new("test content").unwrap();
    let msg = Message::user(content);
    assert_eq!(msg.content(), "test content");
}

#[test]
fn try_user_with_valid_content() {
    let msg = Message::try_user("hello").unwrap();
    assert_eq!(msg.content(), "hello");
    assert_eq!(msg.role_str(), "user");
}

#[test]
fn try_user_with_empty_content() {
    let result = Message::try_user("");
    assert!(result.is_err());
}

#[test]
fn assistant_message_has_provider() {
    let content = NonEmptyString::new("response").unwrap();
    let model = Provider::OpenAI.default_model();

    if let Message::Assistant(m) = Message::assistant(model, content) {
        assert_eq!(m.provider(), Provider::OpenAI);
    } else {
        panic!("Expected Assistant variant");
    }
}

#[test]
fn non_empty_string_deref() {
    let s = NonEmptyString::new("hello").unwrap();
    // Can use str methods via Deref
    assert_eq!(s.len(), 5);
    assert!(s.starts_with("he"));
}

#[test]
fn non_empty_string_try_from_string() {
    let result: Result<NonEmptyString, _> = "test".to_string().try_into();
    assert!(result.is_ok());

    let result: Result<NonEmptyString, _> = String::new().try_into();
    assert!(result.is_err());
}

#[test]
fn non_empty_string_try_from_str() {
    let result: Result<NonEmptyString, _> = "test".try_into();
    assert!(result.is_ok());
}
