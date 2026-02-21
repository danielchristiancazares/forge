#[test]
fn tool_call_accumulator_legacy_shape_fails_to_compile() {
    let t = trybuild::TestCases::new();
    t.compile_fail("tests/compile_fail/tool_call_accumulator_args_exceeded.rs");
}
