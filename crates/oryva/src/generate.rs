//! `oryva generate`: emit into a staging dir, then sync into the real output dirs.

use std::path::{Path, PathBuf};

use crate::config::{Kind, Plan};
use crate::diff;
use crate::sync::{IR_FILE, Sync};
use crate::{Result, USAGE};

pub fn run(args: &[String]) -> Result<()> {
    let (mut spec, mut out, mut config, mut force) = (None, None, None, false);
    let mut args = args.iter();
    while let Some(a) = args.next() {
        match a.as_str() {
            "--out" => out = Some(PathBuf::from(args.next().ok_or("--out needs a directory")?)),
            "--config" => config = Some(PathBuf::from(args.next().ok_or("--config needs a path")?)),
            "--force" => force = true,
            _ if spec.is_none() && !a.starts_with('-') => spec = Some(a),
            _ => return Err(format!("unexpected argument `{a}`\n\n{USAGE}").into()),
        }
    }
    let plan = match (spec, config) {
        (Some(_), Some(_)) => return Err("give a spec or --config, not both".into()),
        (Some(spec), None) => Plan::from_spec(Path::new(spec), out)?,
        (None, config) => {
            if out.is_some() {
                return Err("--out only works with a spec; set output paths in the config".into());
            }
            Plan::from_config(&config.unwrap_or_else(|| "oryva.yml".into()))?
        }
    };
    let api = &plan.api;

    // Check every target before writing any of them.
    let stage = tempfile::tempdir()?;
    let mut syncs = Vec::new();
    for (i, t) in plan.targets.iter().enumerate() {
        let dir = stage.path().join(i.to_string());
        match &t.kind {
            Kind::Python(o) => oryva_emit_python::emit_sdk(api, o, &dir).map(drop)?,
            Kind::Mcp(o) => oryva_emit_python::emit_mcp(api, o, &dir).map(drop)?,
            Kind::TypeScript(o) => oryva_emit_typescript::emit_sdk(api, o, &dir).map(drop)?,
        }
        syncs.push(Sync::check(&dir, &t.out, api, force)?);
    }

    let previous = plan
        .targets
        .iter()
        .map(|t| t.out.join(IR_FILE))
        .find(|p| p.exists())
        .and_then(|p| diff::load(&p).ok());

    for (t, sync) in plan.targets.iter().zip(syncs) {
        let (files, stale) = (sync.file_count(), sync.stale_count());
        sync.apply()?;
        let removed = if stale > 0 {
            format!(", {stale} removed")
        } else {
            String::new()
        };
        println!("{}: {files} files in {}{removed}", t.what, t.out.display());
    }
    if let Some(old) = previous {
        let [b, m, p] = diff::counts(&diff::diff(&old, api));
        if b + m + p == 0 {
            println!("No API changes since the last run.");
        } else {
            println!(
                "API changes since the last run: {b} breaking, {m} minor, {p} patch (see `oryva diff`)."
            );
        }
    }
    Ok(())
}
