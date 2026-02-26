#[test]
fn api_disallows_raw_execute_entrypoint() {
    let t = trybuild::TestCases::new();
    t.compile_fail("tests/trybuild/execute-entrypoint.rs");
    t.compile_fail("tests/trybuild/execute-checked-entrypoint.rs");
    t.compile_fail("tests/trybuild/get-url-entrypoint.rs");
    t.compile_fail("tests/trybuild/get-entrypoint.rs");
    t.compile_fail("tests/trybuild/post-entrypoint.rs");
    t.compile_fail("tests/trybuild/post-json-entrypoint.rs");
    t.compile_fail("tests/trybuild/post-json-direct-entrypoint.rs");
    t.compile_fail("tests/trybuild/post-json-checked-direct-entrypoint.rs");
    t.compile_fail("tests/trybuild/post-json-response-entrypoint.rs");
    t.compile_fail("tests/trybuild/non-bytes-body-entrypoint.rs");
}
