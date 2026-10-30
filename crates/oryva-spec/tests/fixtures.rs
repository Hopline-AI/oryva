use std::path::Path;

fn parse(name: &str) -> oryva_ir::Api {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/specs");
    oryva_spec::parse_file(&path.join(name)).unwrap()
}

#[test]
fn sarvam_lite() {
    insta::assert_json_snapshot!(parse("sarvam-lite.yaml"));
}

#[test]
fn petstore_3_0() {
    insta::assert_json_snapshot!(parse("petstore-3.0.yaml"));
}
