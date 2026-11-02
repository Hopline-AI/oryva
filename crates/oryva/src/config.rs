//! `oryva.yml`: what to generate and where it goes.

use std::path::{Path, PathBuf};

use oryva_ir::Api;
use serde::Deserialize;
use serde_json::Value;

use crate::Result;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct File {
    spec: String,
    name: Option<String>,
    version: Option<String>,
    targets: Option<Targets>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Targets {
    python: Option<PythonCfg>,
    mcp: Option<McpCfg>,
    typescript: Option<TypeScriptCfg>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PythonCfg {
    out: Option<PathBuf>,
    package: Option<String>,
    dist: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct McpCfg {
    out: Option<PathBuf>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TypeScriptCfg {
    out: Option<PathBuf>,
    package: Option<String>,
}

pub enum Kind {
    Python(oryva_emit_python::Options),
    Mcp(oryva_emit_python::Options),
    TypeScript(oryva_emit_typescript::Options),
}

pub struct Target {
    pub what: &'static str,
    pub out: PathBuf,
    pub kind: Kind,
}

/// The parsed API and the targets to generate.
pub struct Plan {
    pub api: Api,
    pub targets: Vec<Target>,
}

impl Plan {
    /// `oryva generate <spec> [--out <dir>]`: all three targets under `<dir>`.
    pub fn from_spec(spec: &Path, out: Option<PathBuf>) -> Result<Plan> {
        let api = oryva_spec::parse_file(spec)?;
        let out = out.unwrap_or_else(|| Path::new("out").join(&api.name));
        let py = oryva_emit_python::Options::for_api(&api);
        let ts = oryva_emit_typescript::Options::for_api(&api);
        let targets = vec![
            python_target(out.join("python"), py.clone()),
            mcp_target(out.join("mcp"), py),
            ts_target(out.join("typescript"), ts),
        ];
        Ok(Plan { api, targets })
    }

    /// `oryva generate [--config <path>]`. Paths in the file are relative to the file.
    pub fn from_config(path: &Path) -> Result<Plan> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| format!("reading {}: {e}", path.display()))?;
        let file: File =
            serde_saphyr::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
        let base = path.parent().unwrap_or(Path::new(""));
        let api = load_spec(&base.join(&file.spec), file.name.as_deref())?;

        let mut py = oryva_emit_python::Options::for_api(&api);
        let mut ts = oryva_emit_typescript::Options::for_api(&api);
        if let Some(v) = &file.version {
            py.version = v.clone();
            ts.version = v.clone();
        }
        let at = |out: &Option<PathBuf>, dir: &str| match out {
            Some(p) => base.join(p),
            None => base.join("out").join(&api.name).join(dir),
        };

        let t = file.targets.unwrap_or(Targets {
            python: Some(PythonCfg {
                out: None,
                package: None,
                dist: None,
            }),
            mcp: Some(McpCfg { out: None }),
            typescript: Some(TypeScriptCfg {
                out: None,
                package: None,
            }),
        });
        if let Some(c) = &t.python {
            if let Some(p) = &c.package {
                py.package = p.clone();
                py.dist_name = p.replace('_', "-");
            }
            if let Some(d) = &c.dist {
                py.dist_name = d.clone();
            }
        }
        let mut targets = Vec::new();
        if let Some(c) = &t.python {
            targets.push(python_target(at(&c.out, "python"), py.clone()));
        }
        if let Some(c) = &t.mcp {
            targets.push(mcp_target(at(&c.out, "mcp"), py));
        }
        if let Some(c) = &t.typescript {
            if let Some(p) = &c.package {
                ts.package = p.clone();
            }
            targets.push(ts_target(at(&c.out, "typescript"), ts));
        }
        Ok(Plan { api, targets })
    }
}

fn python_target(out: PathBuf, o: oryva_emit_python::Options) -> Target {
    Target {
        what: "Python SDK",
        out,
        kind: Kind::Python(o),
    }
}

fn mcp_target(out: PathBuf, o: oryva_emit_python::Options) -> Target {
    Target {
        what: "MCP server",
        out,
        kind: Kind::Mcp(o),
    }
}

fn ts_target(out: PathBuf, o: oryva_emit_typescript::Options) -> Target {
    Target {
        what: "TypeScript SDK",
        out,
        kind: Kind::TypeScript(o),
    }
}

/// Parses a spec. A `name` replaces `info.x-oryva-name` before parsing, so env var names follow it.
fn load_spec(path: &Path, name: Option<&str>) -> Result<Api> {
    let Some(name) = name else {
        return Ok(oryva_spec::parse_file(path)?);
    };
    let src =
        std::fs::read_to_string(path).map_err(|e| format!("reading {}: {e}", path.display()))?;
    let mut doc: Value = if src.trim_start().starts_with('{') {
        serde_json::from_str(&src)?
    } else {
        let mut options = serde_saphyr::Options::default();
        options.strict_booleans = true;
        options.budget = None;
        serde_saphyr::from_str_with_options(&src, options)?
    };
    if let Some(info) = doc.get_mut("info").and_then(Value::as_object_mut) {
        info.insert("x-oryva-name".into(), name.into());
    }
    Ok(oryva_spec::parse_str(&serde_json::to_string(&doc)?)?)
}
