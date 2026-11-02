mod config;
mod diff;
mod generate;
mod sync;

use std::path::Path;
use std::process::ExitCode;

use oryva_eval::{HttpModel, TaskFile, lint, run};
use serde_json::json;

pub const USAGE: &str = "\
usage:
  oryva generate <spec.yaml|spec.json> [--out <dir>] [--force]
      Write a Python SDK to <dir>/python, its MCP server to <dir>/mcp and a
      TypeScript SDK to <dir>/typescript. <dir> defaults to out/<api name>.
  oryva generate [--config <oryva.yml>] [--force]
      The same, with the spec, names, version and output paths from the config
      (default ./oryva.yml). Generated files you edited by hand abort the run
      unless --force.
  oryva diff <old> <new> [--version <current>] [--json]
      Report breaking, minor and patch changes between two specs or ir.json
      files; with --version, also the next semver version.
  oryva eval <tasks.yaml> [--lint-only] [--json <file>]
      Lint the MCP server's tools, then run the tasks with Claude.
      Running tasks needs ANTHROPIC_API_KEY; --lint-only needs no key.

Exit codes: 0 ok, 1 breaking changes, lint errors or failed tasks, 2 usage or runtime error.";

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

#[tokio::main]
async fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("generate") => generate::run(&args[1..]).map(|()| true),
        Some("diff") => diff(&args[1..]),
        Some("eval") => eval(&args[1..]).await,
        Some("help" | "-h" | "--help") => {
            println!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        _ => Err(USAGE.into()),
    };
    match result {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::from(1),
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::from(2)
        }
    }
}

/// Returns false when there are breaking changes.
fn diff(args: &[String]) -> Result<bool> {
    let (mut sides, mut version, mut json) = (Vec::new(), None, false);
    let mut args = args.iter();
    while let Some(a) = args.next() {
        match a.as_str() {
            "--json" => json = true,
            "--version" => version = Some(args.next().ok_or("--version needs a value")?),
            _ if !a.starts_with('-') => sides.push(a),
            _ => return Err(format!("unexpected argument `{a}`\n\n{USAGE}").into()),
        }
    }
    let [old, new] = sides[..] else {
        return Err(format!("oryva diff needs <old> and <new>\n\n{USAGE}").into());
    };
    let changes = diff::diff(&diff::load(Path::new(old))?, &diff::load(Path::new(new))?);
    let top = diff::top(&changes);
    let next = match version {
        Some(v) => Some((v.as_str(), diff::next_version(v, top)?)),
        None => None,
    };
    let next = next.as_ref().map(|(v, n)| (*v, n.as_str()));
    if json {
        let report = diff::json_report(&changes, next);
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print!("{}", diff::markdown(&changes, next));
    }
    Ok(top != Some(diff::Severity::Breaking))
}

/// Returns false when the lint found errors or a task failed.
async fn eval(args: &[String]) -> Result<bool> {
    let mut tasks = None;
    let mut lint_only = false;
    let mut json_out = None;
    let mut args = args.iter();
    while let Some(a) = args.next() {
        match a.as_str() {
            "--lint-only" => lint_only = true,
            "--json" => json_out = Some(args.next().ok_or("--json needs a file path")?),
            _ if tasks.is_none() && !a.starts_with('-') => tasks = Some(a),
            _ => return Err(format!("unexpected argument `{a}`\n\n{USAGE}").into()),
        }
    }
    let file = TaskFile::load(tasks.ok_or(USAGE)?)?;

    let lint_report = lint(&file.server).await?;
    println!("{lint_report}");
    let mut ok = lint_report.error_count() == 0;
    let mut report = json!({ "lint": lint_report });

    if !lint_only {
        let model = HttpModel::from_env()?;
        let run_report = run(&file, &model).await?;
        println!("{run_report}");
        ok &= run_report.summary.passed == run_report.summary.tasks;
        report["run"] = serde_json::to_value(&run_report)?;
    }
    if let Some(path) = json_out {
        std::fs::write(path, serde_json::to_string_pretty(&report)?)?;
    }
    Ok(ok)
}
