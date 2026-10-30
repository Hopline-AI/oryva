use std::path::{Path, PathBuf};
use std::process::Command;

use oryva_emit_python::{Options, emit_mcp, emit_sdk};
use oryva_ir::{Api, ResponseBody};

fn fixture() -> Api {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/sarvam-lite.ir.json");
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

fn snapshot_all(root: &Path, files: &[PathBuf], prefix: &str) {
    for file in files {
        let rel = file
            .strip_prefix(root)
            .unwrap()
            .to_string_lossy()
            .replace('/', "__");
        let content = std::fs::read_to_string(file).unwrap();
        insta::with_settings!({prepend_module_to_snapshot => false}, {
            insta::assert_snapshot!(format!("{prefix}__{rel}"), content);
        });
    }
}

fn check_python(dir: &Path) {
    let ruff = Command::new("ruff")
        .args(["check", "--no-cache"])
        .arg(dir)
        .output()
        .unwrap();
    assert!(
        ruff.status.success(),
        "ruff check:\n{}",
        String::from_utf8_lossy(&ruff.stdout)
    );
    let compile = Command::new("python3")
        .args(["-m", "compileall", "-q"])
        .arg(dir)
        .output()
        .unwrap();
    assert!(
        compile.status.success(),
        "compileall:\n{}",
        String::from_utf8_lossy(&compile.stdout)
    );
}

#[test]
fn sdk_snapshots_and_lints() {
    let api = fixture();
    let dir = tempfile::tempdir().unwrap();
    let files = emit_sdk(&api, &Options::for_api(&api), dir.path()).unwrap();
    snapshot_all(dir.path(), &files, "sdk");
    check_python(dir.path());
}

#[test]
fn mcp_snapshots_and_lints() {
    let api = fixture();
    let dir = tempfile::tempdir().unwrap();
    let files = emit_mcp(&api, &Options::for_api(&api), dir.path()).unwrap();
    snapshot_all(dir.path(), &files, "mcp");
    check_python(dir.path());
}

/// The fixture has no binary, text or empty-body paths that differ from JSON; swap some in.
#[test]
fn binary_text_and_empty_responses_compile() {
    let mut api = fixture();
    let methods = &mut api.resources[0].methods;
    methods[0].response = ResponseBody::Binary {
        content_type: "audio/wav".into(),
    };
    methods[1].response = ResponseBody::Text;
    methods[2].response = ResponseBody::Empty;
    let opts = Options::for_api(&api);
    let dir = tempfile::tempdir().unwrap();
    emit_sdk(&api, &opts, &dir.path().join("sdk")).unwrap();
    emit_mcp(&api, &opts, &dir.path().join("mcp")).unwrap();
    check_python(dir.path());
    let server = std::fs::read_to_string(dir.path().join("mcp/src/sarvam_mcp/server.py")).unwrap();
    assert!(server.contains("_save_binary(result, \"text_translate\", \"audio/wav\")"));
}

#[test]
fn default_options_follow_api_name() {
    let mut api = fixture();
    api.name = "my_api".into();
    let opts = Options::for_api(&api);
    assert_eq!(
        (opts.package.as_str(), opts.dist_name.as_str()),
        ("my_api", "my-api")
    );
    assert_eq!(opts.version, "1.0");
}
