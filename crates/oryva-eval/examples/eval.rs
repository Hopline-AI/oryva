//! `cargo run -p oryva-eval --example eval -- <task.yaml> [--lint-only] [--json out.json]`

use std::process::ExitCode;

use oryva_eval::{HttpModel, TaskFile, lint, run};
use serde_json::json;

#[tokio::main]
async fn main() -> ExitCode {
    match go().await {
        Ok(ok) => ExitCode::from(if ok { 0 } else { 1 }),
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::from(2)
        }
    }
}

async fn go() -> Result<bool, Box<dyn std::error::Error>> {
    let mut path = None;
    let mut lint_only = false;
    let mut json_out = None;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--lint-only" => lint_only = true,
            "--json" => json_out = Some(args.next().ok_or("--json needs a path")?),
            _ if path.is_none() => path = Some(a),
            _ => return Err(format!("unexpected argument `{a}`").into()),
        }
    }
    let file =
        TaskFile::load(path.ok_or("usage: eval <task.yaml> [--lint-only] [--json out.json]")?)?;

    let lint_report = lint(&file.server).await?;
    println!("{lint_report}");
    let mut ok = lint_report.error_count() == 0;
    let mut out = json!({"lint": lint_report});

    if !lint_only {
        let model = HttpModel::from_env()?;
        let report = run(&file, &model).await?;
        println!("{report}");
        ok &= report.summary.passed == report.summary.tasks;
        out["run"] = serde_json::to_value(&report)?;
    }
    if let Some(p) = json_out {
        std::fs::write(&p, serde_json::to_string_pretty(&out)?)?;
    }
    Ok(ok)
}
