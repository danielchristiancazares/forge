#[test]
fn removed_predicate_helpers_fail_to_compile() {
    let t = trybuild::TestCases::new();
    t.compile_fail("tests/compile_fail/predicate_helpers_removed.rs");
}
