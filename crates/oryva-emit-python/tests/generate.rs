use std::path::{Path, PathBuf};
use std::process::Command;

use oryva_emit_python::{Options, emit_mcp, emit_sdk};
use oryva_ir::{Api, ApiKeyLocation, Auth, ResponseBody};

fn fixture() -> Api {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/ledger.ir.json");
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

/// The fixture has no text response, no API key or basic auth, and no APIs without servers,
/// idempotency or webhooks; swap those in and make sure the output still compiles.
#[test]
fn other_response_kinds_auth_schemes_and_bare_apis_compile() {
    let mut api = fixture();
    api.resources[0].methods[0].response = ResponseBody::Text;
    api.resources[0].methods[0].pagination = None;
    api.auth = vec![
        Auth::ApiKey {
            location: ApiKeyLocation::Header,
            name: "X-Api-Key".into(),
            env_var: "LEDGER_API_KEY".into(),
        },
        Auth::ApiKey {
            location: ApiKeyLocation::Query,
            name: "key".into(),
            env_var: "LEDGER_QUERY_KEY".into(),
        },
        Auth::ApiKey {
            location: ApiKeyLocation::Cookie,
            name: "session".into(),
            env_var: "LEDGER_SESSION".into(),
        },
        Auth::Basic {
            username_env: "LEDGER_USERNAME".into(),
            password_env: "LEDGER_PASSWORD".into(),
        },
    ];
    let opts = Options::for_api(&api);
    let dir = tempfile::tempdir().unwrap();
    emit_sdk(&api, &opts, &dir.path().join("variants/sdk")).unwrap();
    emit_mcp(&api, &opts, &dir.path().join("variants/mcp")).unwrap();
    let client =
        std::fs::read_to_string(dir.path().join("variants/sdk/src/ledger/_client.py")).unwrap();
    for needle in [
        "api_key_auth(\"header\", \"X-Api-Key\"",
        "api_key_auth(\"query\", \"key\"",
        "api_key_auth(\"cookie\", \"session\"",
        "basic_auth(username, password)",
    ] {
        assert!(client.contains(needle), "missing {needle}");
    }

    api.servers.clear();
    api.auth.clear();
    api.idempotency_header = None;
    api.webhooks.clear();
    api.webhook_signing = None;
    emit_sdk(&api, &opts, &dir.path().join("bare/sdk")).unwrap();
    emit_mcp(&api, &opts, &dir.path().join("bare/mcp")).unwrap();
    check_python(dir.path());
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
    // PyPI rejects `2026-10-01`.
    assert_eq!(opts.version, "2026.10.01");
    api.version = "1.4".into();
    assert_eq!(Options::for_api(&api).version, "1.4");
}

fn edge_cases() -> Api {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/specs/edge-cases.yaml");
    oryva_spec::parse_file(&path).unwrap()
}

/// Keywords, odd characters, name collisions and nasty descriptions from real specs. `ruff` and
/// `compileall` reject any of them emitted bare.
#[test]
fn edge_cases_compile() {
    let api = edge_cases();
    let opts = Options::for_api(&api);
    let dir = tempfile::tempdir().unwrap();
    emit_sdk(&api, &opts, &dir.path().join("sdk")).unwrap();
    emit_mcp(&api, &opts, &dir.path().join("mcp")).unwrap();
    check_python(dir.path());

    let read = |p: &str| std::fs::read_to_string(dir.path().join("sdk/src/edge").join(p)).unwrap();
    let types = read("types.py");
    // A keyword is escaped in code and kept on the wire through an alias.
    assert!(types.contains("async_: Optional[bool] = Field(None, alias=\"async\")"));
    assert!(types.contains("from_: Optional[str] = Field(None, alias=\"from\")"));
    // Fields that would override a pydantic member, or collide after snake_casing, are renamed.
    assert!(types.contains("schema_: Optional[Dict[str, Any]] = Field(None, alias=\"schema\")"));
    assert!(types.contains("field_1: Optional[int] = Field(None, alias=\"+1\")"));
    assert!(types.contains("field_: Optional[str] = Field(None, alias=\"\")"));
    assert!(types.contains("field_1_2: Optional[int] = Field(None, alias=\"-1\")"));
    // Models named like the generated code's own imports get a suffix.
    assert!(types.contains("class PageModel(BaseModel)"));
    // An object default is built into its model.
    assert!(types.contains("default_factory=lambda: Settings.model_validate({\"verbose\": True})"));

    // A union that contains itself cannot name itself: pydantic recurses forever on that.
    assert!(types.contains("Details = Union[Dict[str, Any], Leaf]"));

    let items = read("resources/items.py");
    // Colliding and reserved parameter names are numbered, and the wire names stay.
    assert!(items.contains("timeout_2: int | NotGiven"));
    assert!(items.contains("\"timeout\": timeout_2"));
    assert!(items.contains("import_: str,"));
    // The body's content type is sent as declared.
    assert!(items.contains("content_type=\"image/jpeg\""));
    // A path parameter called `quote` cannot shadow the URL-quoting helper.
    assert!(items.contains("_quote(quote, safe="));
    // A resource whose name is also a model gets a `Resource` suffix.
    assert!(read("resources/workflows.py").contains("class WorkflowsResource:"));
}
