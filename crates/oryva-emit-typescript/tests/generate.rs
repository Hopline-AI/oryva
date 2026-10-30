use std::path::{Path, PathBuf};
use std::process::Command;

use oryva_emit_typescript::{Options, emit_sdk};
use oryva_ir::Api;

/// Matches scripts/smoke-typescript.sh.
const TYPESCRIPT: &str = "typescript@7.0.2";

fn fixture() -> Api {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/ledger.ir.json");
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

fn snapshot_all(root: &Path, files: &[PathBuf]) {
    for file in files {
        let rel = file
            .strip_prefix(root)
            .unwrap()
            .to_string_lossy()
            .replace('/', "__");
        let content = std::fs::read_to_string(file).unwrap();
        insta::with_settings!({prepend_module_to_snapshot => false}, {
            insta::assert_snapshot!(format!("sdk__{rel}"), content);
        });
    }
}

/// Runs `tsc --noEmit` on a generated package. Skips when Node isn't installed; needs the
/// network the first time for `npx` to fetch TypeScript.
fn type_check(dir: &Path) {
    if Command::new("node").arg("--version").output().is_err() {
        eprintln!("node not found; skipping tsc");
        return;
    }
    let output = Command::new("npx")
        .args(["--yes", "-p", TYPESCRIPT, "tsc", "--noEmit"])
        .current_dir(dir)
        .output()
        .expect("run npx");
    assert!(
        output.status.success(),
        "tsc --noEmit failed:\n{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn sdk_snapshots_and_type_checks() {
    let api = fixture();
    let dir = tempfile::tempdir().unwrap();
    let files = emit_sdk(&api, &Options::for_api(&api), dir.path()).unwrap();
    snapshot_all(dir.path(), &files);
    type_check(dir.path());
}

/// Auth schemes, parameter locations and body encodings the Ledger fixture doesn't use.
#[test]
fn other_auth_schemes_and_encodings_type_check() {
    let mut json: serde_json::Value = serde_json::to_value(fixture()).unwrap();
    json["auth"] = serde_json::json!([
        {"kind": "api_key", "location": "query", "name": "api_key", "env_var": "LEDGER_QUERY_KEY"},
        {"kind": "api_key", "location": "header", "name": "x-api-key", "env_var": "LEDGER_API_KEY"},
        {"kind": "api_key", "location": "cookie", "name": "session", "env_var": "LEDGER_SESSION"},
        {"kind": "basic", "username_env": "LEDGER_USERNAME", "password_env": "LEDGER_PASSWORD"}
    ]);
    json["idempotency_header"] = serde_json::Value::Null;
    json["webhook_signing"] = serde_json::Value::Null;
    json["resources"][0]["methods"].as_array_mut().unwrap().push(serde_json::json!({
        "name": "import", "http_method": "POST", "path": "/customers/import/{kind}",
        "summary": null, "description": null,
        "params": [
            {"name": "kind", "ident": "kind", "location": "path", "required": true, "ty": {"kind": "string", "format": null}, "description": null, "default": null},
            {"name": "X-Dry-Run", "ident": "x_dry_run", "location": "header", "required": false, "ty": {"kind": "boolean"}, "description": null, "default": null},
            {"name": "mode", "ident": "mode", "location": "query", "required": false, "ty": {"kind": "string", "format": null}, "description": null, "default": null}
        ],
        "body": {"encoding": "form", "required": true, "ty": {"kind": "named", "name": "CustomersCreateRequest"}},
        "response": {"kind": "text"}, "pagination": null, "errors": [], "deprecated": false,
        "agent": {"read_only": false, "destructive": false, "idempotent": false, "hint": null, "examples": []}
    }));
    let api: Api = serde_json::from_value(json).unwrap();

    let dir = tempfile::tempdir().unwrap();
    emit_sdk(&api, &Options::for_api(&api), dir.path()).unwrap();
    let client = std::fs::read_to_string(dir.path().join("src/client.ts")).unwrap();
    assert!(client.contains("request.query['api_key'] = this.queryKey"));
    assert!(client.contains("request.headers.set('x-api-key', this.apiKey)"));
    assert!(client.contains("request.headers.append('cookie', `session=${this.session}`)"));
    assert!(client.contains("Basic ${base64("));
    assert!(!dir.path().join("src/oauth.ts").exists());
    assert!(!dir.path().join("src/webhooks.ts").exists());
    type_check(dir.path());
}

#[test]
fn package_version_is_semver() {
    let mut api = fixture();
    assert_eq!(Options::for_api(&api).version, "2026.10.1");
    api.version = "1.4.2".into();
    assert_eq!(Options::for_api(&api).version, "1.4.2");
    api.version = "v2-beta".into();
    assert_eq!(Options::for_api(&api).version, "0.1.0");
}
