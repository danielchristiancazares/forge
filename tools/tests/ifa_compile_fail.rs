#[test]
fn removed_tool_executor_methods_fail_to_compile() {
    let t = trybuild::TestCases::new();
    t.compile_fail("tests/compile_fail/tool_executor_legacy_methods.rs");
}
