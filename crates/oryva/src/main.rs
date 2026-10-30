use std::path::{Path, PathBuf};
use std::process::ExitCode;

use oryva_emit_python::{Options, emit_mcp, emit_sdk};
use oryva_eval::{HttpModel, TaskFile, lint, run};
use serde_json::json;

const USAGE: &str = "\
usage:
  oryva generate <spec.yaml|spec.json> [--out <dir>]
      Write a Python SDK to <dir>/sdk and an MCP server to <dir>/mcp.
      <dir> defaults to out/<api name>.
  oryva eval <tasks.yaml> [--lint-only] [--json <file>]
      Lint the MCP server's tools, then run the tasks with Claude.
      Running tasks needs ANTHROPIC_API_KEY; --lint-only needs no key.

Exit codes: 0 ok, 1 lint errors or failed tasks, 2 usage or runtime error.";

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

#[tokio::main]
async fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("generate") => generate(&args[1..]).map(|()| true),
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

fn generate(args: &[String]) -> Result<()> {
    let mut spec = None;
    let mut out = None;
    let mut args = args.iter();
    while let Some(a) = args.next() {
        match a.as_str() {
            "--out" => out = Some(PathBuf::from(args.next().ok_or("--out needs a directory")?)),
            _ if spec.is_none() && !a.starts_with('-') => spec = Some(a),
            _ => return Err(format!("unexpected argument `{a}`\n\n{USAGE}").into()),
        }
    }
    let api = oryva_spec::parse_file(Path::new(spec.ok_or(USAGE)?))?;
    let out = out.unwrap_or_else(|| Path::new("out").join(&api.name));
    let opts = Options::for_api(&api);
    let (sdk_dir, mcp_dir) = (out.join("sdk"), out.join("mcp"));
    let sdk = emit_sdk(&api, &opts, &sdk_dir)?;
    let mcp = emit_mcp(&api, &opts, &mcp_dir)?;
    println!(
        "wrote {} SDK files to {} and {} MCP server files to {}",
        sdk.len(),
        sdk_dir.display(),
        mcp.len(),
        mcp_dir.display()
    );
    Ok(())
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
