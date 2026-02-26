#[test]
fn api_disallows_raw_execute_entrypoint() {
    let t = trybuild::TestCases::new();
    t.compile_fail("tests/trybuild/execute-entrypoint.rs");
}
