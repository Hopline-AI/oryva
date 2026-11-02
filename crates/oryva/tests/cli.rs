//! The CLI as a user runs it: config, regeneration and `oryva diff`, against temp directories.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const LEDGER: &str = include_str!("../../../fixtures/specs/ledger.yaml");

fn oryva(dir: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_oryva"))
        .current_dir(dir)
        .args(args)
        .output()
        .unwrap()
}

fn stdout(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}

fn stderr(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

fn code(o: &Output) -> i32 {
    o.status.code().unwrap()
}

fn ok(o: &Output) {
    assert_eq!(code(o), 0, "stdout: {}\nstderr: {}", stdout(o), stderr(o));
}

/// ledger.yaml without its `/refunds` path: the `refunds` resource disappears.
fn without_refunds() -> String {
    let start = LEDGER.find("\n  /refunds:\n").unwrap();
    let end = LEDGER.find("\n  /files:\n").unwrap();
    format!("{}{}", &LEDGER[..start], &LEDGER[end..])
}

fn write(dir: &Path, rel: &str, text: &str) -> PathBuf {
    let path = dir.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, text).unwrap();
    path
}

/// A project with `openapi.yaml` and a config that only builds the TypeScript SDK.
fn ts_project(spec: &str) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "openapi.yaml", spec);
    write(
        dir.path(),
        "oryva.yml",
        "spec: openapi.yaml\ntargets:\n  typescript: { out: sdk }\n",
    );
    dir
}

#[test]
fn config_names_versions_and_paths_are_respected() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "specs/api.yaml", LEDGER);
    write(
        dir.path(),
        "conf/oryva.yml",
        "spec: ../specs/api.yaml\nversion: 1.4.0\ntargets:\n  \
         python: { out: ../sdks/py, package: acme_ledger, dist: acme-ledger-sdk }\n  \
         mcp: { out: ../sdks/mcp }\n  \
         typescript: { out: ../sdks/ts, package: \"@acme/ledger\" }\n",
    );
    ok(&oryva(
        dir.path(),
        &["generate", "--config", "conf/oryva.yml"],
    ));

    let pyproject = fs::read_to_string(dir.path().join("sdks/py/pyproject.toml")).unwrap();
    assert!(
        pyproject.contains("name = \"acme-ledger-sdk\""),
        "{pyproject}"
    );
    assert!(pyproject.contains("version = \"1.4.0\""), "{pyproject}");
    assert!(
        dir.path()
            .join("sdks/py/src/acme_ledger/__init__.py")
            .exists()
    );
    let package = fs::read_to_string(dir.path().join("sdks/ts/package.json")).unwrap();
    assert!(package.contains("\"name\": \"@acme/ledger\""), "{package}");
    assert!(package.contains("\"version\": \"1.4.0\""), "{package}");
    assert!(dir.path().join("sdks/mcp/pyproject.toml").exists());
}

#[test]
fn omitted_targets_are_skipped() {
    let dir = ts_project(LEDGER);
    ok(&oryva(dir.path(), &["generate"]));
    assert!(dir.path().join("sdk/package.json").exists());
    let entries: Vec<_> = fs::read_dir(dir.path()).unwrap().collect();
    assert_eq!(
        entries.len(),
        3,
        "only openapi.yaml, oryva.yml and sdk: {entries:?}"
    );
}

#[test]
fn no_targets_key_builds_all_three_under_out_name() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "openapi.yaml", LEDGER);
    write(dir.path(), "oryva.yml", "spec: openapi.yaml\n");
    ok(&oryva(dir.path(), &["generate"]));
    for d in ["python", "mcp", "typescript"] {
        assert!(
            dir.path()
                .join("out/ledger")
                .join(d)
                .join(".oryva/manifest.json")
                .exists()
        );
    }
}

#[test]
fn unknown_config_keys_are_errors() {
    let dir = ts_project(LEDGER);
    write(
        dir.path(),
        "oryva.yml",
        "spec: openapi.yaml\ntargets:\n  typescript: { out: sdk, pakage: x }\n",
    );
    let o = oryva(dir.path(), &["generate"]);
    assert_eq!(code(&o), 2);
    assert!(stderr(&o).contains("pakage"), "{}", stderr(&o));
    assert!(!dir.path().join("sdk").exists());
}

#[test]
fn spec_and_out_still_work_without_a_config() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "ledger.yaml", LEDGER);
    ok(&oryva(
        dir.path(),
        &["generate", "ledger.yaml", "--out", "o"],
    ));
    assert!(dir.path().join("o/python/pyproject.toml").exists());
    assert!(dir.path().join("o/mcp/pyproject.toml").exists());
    assert!(dir.path().join("o/typescript/package.json").exists());
}

#[test]
fn regenerating_deletes_files_for_removed_methods() {
    let dir = ts_project(LEDGER);
    ok(&oryva(dir.path(), &["generate"]));
    let refunds = dir.path().join("sdk/src/resources/refunds.ts");
    assert!(refunds.exists());

    write(dir.path(), "openapi.yaml", &without_refunds());
    let o = oryva(dir.path(), &["generate"]);
    ok(&o);
    assert!(!refunds.exists());
    assert!(dir.path().join("sdk/src/resources/customers.ts").exists());
    let index = fs::read_to_string(dir.path().join("sdk/src/index.ts")).unwrap();
    assert!(!index.contains("refunds"), "{index}");
    assert!(stdout(&o).contains("1 removed"), "{}", stdout(&o));
    assert!(stdout(&o).contains("2 breaking"), "{}", stdout(&o));
}

#[test]
fn empty_directories_are_removed_with_their_last_file() {
    let dir = ts_project(LEDGER);
    ok(&oryva(dir.path(), &["generate"]));
    let manifest = dir.path().join("sdk/.oryva/manifest.json");
    // Pretend an older run generated a file in a nested directory that is no longer produced.
    write(dir.path(), "sdk/old/deep/legacy.ts", "export {};\n");
    let text = fs::read_to_string(&manifest).unwrap();
    let hash = {
        use sha2::{Digest, Sha256};
        format!("{:x}", Sha256::digest(b"export {};\n"))
    };
    let text = text.replacen(
        "\"files\": {",
        &format!("\"files\": {{\n    \"old/deep/legacy.ts\": \"{hash}\","),
        1,
    );
    fs::write(&manifest, text).unwrap();
    ok(&oryva(dir.path(), &["generate"]));
    assert!(!dir.path().join("sdk/old").exists());
    assert!(dir.path().join("sdk/src").exists());
}

#[test]
fn hand_edited_file_aborts_without_writing_and_force_overwrites() {
    let dir = ts_project(LEDGER);
    ok(&oryva(dir.path(), &["generate"]));
    let customers = dir.path().join("sdk/src/resources/customers.ts");
    let original = fs::read_to_string(&customers).unwrap();
    fs::write(&customers, format!("{original}// my change\n")).unwrap();
    let manifest_before = fs::read(dir.path().join("sdk/.oryva/manifest.json")).unwrap();

    // The new spec would also delete refunds.ts and rewrite other files.
    write(dir.path(), "openapi.yaml", &without_refunds());
    let o = oryva(dir.path(), &["generate"]);
    assert_eq!(code(&o), 2);
    let err = stderr(&o);
    assert!(err.contains("src/resources/customers.ts"), "{err}");
    assert!(err.contains("--force"), "{err}");
    assert!(
        fs::read_to_string(&customers)
            .unwrap()
            .ends_with("// my change\n")
    );
    assert!(
        dir.path().join("sdk/src/resources/refunds.ts").exists(),
        "nothing may be deleted"
    );
    assert_eq!(
        fs::read(dir.path().join("sdk/.oryva/manifest.json")).unwrap(),
        manifest_before
    );

    ok(&oryva(dir.path(), &["generate", "--force"]));
    assert_eq!(fs::read_to_string(&customers).unwrap(), original);
    assert!(!dir.path().join("sdk/src/resources/refunds.ts").exists());
}

#[test]
fn files_oryva_did_not_generate_are_never_touched() {
    let dir = ts_project(LEDGER);
    ok(&oryva(dir.path(), &["generate"]));
    write(dir.path(), "sdk/src/custom.ts", "export const mine = 1;\n");
    write(dir.path(), "sdk/notes/todo.md", "keep me\n");

    write(dir.path(), "openapi.yaml", &without_refunds());
    ok(&oryva(dir.path(), &["generate", "--force"]));
    assert_eq!(
        fs::read_to_string(dir.path().join("sdk/src/custom.ts")).unwrap(),
        "export const mine = 1;\n"
    );
    assert_eq!(
        fs::read_to_string(dir.path().join("sdk/notes/todo.md")).unwrap(),
        "keep me\n"
    );
}

#[test]
fn collision_with_an_owned_file_aborts() {
    // Before the first run: the directory already has a package.json of its own.
    let dir = ts_project(LEDGER);
    write(dir.path(), "sdk/package.json", "{\"mine\": true}\n");
    let o = oryva(dir.path(), &["generate", "--force"]);
    assert_eq!(code(&o), 2);
    assert!(stderr(&o).contains("package.json"), "{}", stderr(&o));
    assert_eq!(
        fs::read_to_string(dir.path().join("sdk/package.json")).unwrap(),
        "{\"mine\": true}\n"
    );
    assert!(
        !dir.path().join("sdk/src").exists(),
        "nothing else may be written"
    );

    // After a run: the user's file sits where a newly added resource would go.
    let dir = ts_project(&without_refunds());
    ok(&oryva(dir.path(), &["generate"]));
    let mine = write(dir.path(), "sdk/src/resources/refunds.ts", "// mine\n");
    write(dir.path(), "openapi.yaml", LEDGER);
    let o = oryva(dir.path(), &["generate"]);
    assert_eq!(code(&o), 2);
    assert!(
        stderr(&o).contains("src/resources/refunds.ts"),
        "{}",
        stderr(&o)
    );
    assert_eq!(fs::read_to_string(mine).unwrap(), "// mine\n");
}

fn diff_files(old: &str, new: &str, extra: &[&str]) -> (tempfile::TempDir, Output) {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "old.yaml", old);
    write(dir.path(), "new.yaml", new);
    let mut args = vec!["diff", "old.yaml", "new.yaml"];
    args.extend(extra);
    let o = oryva(dir.path(), &args);
    (dir, o)
}

#[test]
fn diff_reports_breaking_changes_and_the_next_major_version() {
    let (_d, o) = diff_files(LEDGER, &without_refunds(), &["--version", "1.4.0"]);
    assert_eq!(code(&o), 1, "{}", stderr(&o));
    let out = stdout(&o);
    assert!(out.contains("## Breaking"), "{out}");
    assert!(out.contains("- refunds: resource removed"), "{out}");
    assert!(out.contains("Next version: 1.4.0 -> 2.0.0"), "{out}");

    let (_d, o) = diff_files(
        LEDGER,
        &without_refunds(),
        &["--version", "0.3.1", "--json"],
    );
    assert_eq!(code(&o), 1);
    let v: serde_json::Value = serde_json::from_str(&stdout(&o)).unwrap();
    assert_eq!(v["next_version"], "0.4.0");
    assert!(
        v["breaking"]
            .as_array()
            .unwrap()
            .iter()
            .any(|s| s == "refunds: resource removed")
    );
}

#[test]
fn diff_of_additions_exits_zero_with_a_minor_bump() {
    let (_d, o) = diff_files(&without_refunds(), LEDGER, &["--version", "1.4.0"]);
    assert_eq!(code(&o), 0, "{}", stderr(&o));
    let out = stdout(&o);
    assert!(
        out.contains("## Minor") && !out.contains("## Breaking"),
        "{out}"
    );
    assert!(out.contains("- refunds: resource added"), "{out}");
    assert!(out.contains("Next version: 1.4.0 -> 1.5.0"), "{out}");
}

#[test]
fn diff_accepts_an_ir_file_and_errors_exit_two() {
    let dir = ts_project(LEDGER);
    ok(&oryva(dir.path(), &["generate"]));
    write(dir.path(), "new.yaml", &without_refunds());
    let o = oryva(dir.path(), &["diff", "sdk/.oryva/ir.json", "new.yaml"]);
    assert_eq!(code(&o), 1, "{}", stderr(&o));
    assert!(stdout(&o).contains("refunds: resource removed"));

    let same = oryva(dir.path(), &["diff", "sdk/.oryva/ir.json", "openapi.yaml"]);
    ok(&same);
    assert!(stdout(&same).contains("No changes."));

    let o = oryva(dir.path(), &["diff", "missing.yaml", "new.yaml"]);
    assert_eq!(code(&o), 2);
}
