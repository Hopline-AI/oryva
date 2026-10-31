//! Emits a Python SDK (`httpx` + `pydantic` v2) and a Python MCP server from an
//! [`oryva_ir::Api`]. Names, signatures and docstrings are computed in Rust; the templates only
//! lay them out.

mod mcp;
mod py;
mod sdk;

use std::fmt;
use std::path::{Path, PathBuf};
use std::process::Command;

use minijinja::{Environment, context};
use oryva_ir::Api;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Options {
    /// Import name, e.g. `ledger`.
    pub package: String,
    /// Name on PyPI, e.g. `ledger`.
    pub dist_name: String,
    pub version: String,
}

impl Options {
    pub fn for_api(api: &Api) -> Self {
        Options {
            package: api.name.clone(),
            dist_name: api.name.replace('_', "-"),
            version: pep440(&api.version),
        }
    }
}

/// Date versions such as `2026-10-01` aren't valid for PyPI; write them `2026.10.01`.
fn pep440(version: &str) -> String {
    let parts: Vec<&str> = version.split('-').collect();
    let is_date = parts.len() == 3
        && parts.iter().all(|p| p.chars().all(|c| c.is_ascii_digit()))
        && parts[0].len() == 4;
    if is_date {
        parts.join(".")
    } else {
        version.to_string()
    }
}

#[derive(Debug)]
pub enum Error {
    Io(std::io::Error),
    Template(minijinja::Error),
    RuffMissing,
    Ruff(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Io(e) => write!(f, "writing output: {e}"),
            Error::Template(e) => write!(f, "rendering template: {e:#}"),
            Error::RuffMissing => write!(
                f,
                "`ruff` was not found on PATH; it formats the generated Python. \
                 Install it (`brew install ruff` or `pip install ruff`) and run again"
            ),
            Error::Ruff(msg) => write!(f, "`ruff` failed:\n{msg}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error::Io(e)
    }
}

impl From<minijinja::Error> for Error {
    fn from(e: minijinja::Error) -> Self {
        Error::Template(e)
    }
}

macro_rules! templates {
    ($($name:literal),* $(,)?) => {
        &[$(($name, include_str!(concat!("../templates/", $name, ".j2")))),*]
    };
}

const TEMPLATES: &[(&str, &str)] = templates![
    "sdk/pyproject.toml",
    "sdk/README.md",
    "sdk/__init__.py",
    "sdk/_version.py",
    "sdk/_client.py",
    "sdk/_base.py",
    "sdk/_auth.py",
    "sdk/_errors.py",
    "sdk/_utils.py",
    "sdk/_pagination.py",
    "sdk/_streaming.py",
    "sdk/_response.py",
    "sdk/types.py",
    "sdk/resources/__init__.py",
    "sdk/resources/resource.py",
    "sdk/resources/webhooks.py",
    "mcp/pyproject.toml",
    "mcp/README.md",
    "mcp/__init__.py",
    "mcp/server.py",
];

fn env() -> Result<Environment<'static>, Error> {
    let mut env = Environment::new();
    env.set_trim_blocks(true);
    env.set_lstrip_blocks(true);
    env.set_keep_trailing_newline(true);
    env.add_filter("q", |s: String| py::py_str(&s));
    for (name, src) in TEMPLATES {
        env.add_template(name, src)?;
    }
    Ok(env)
}

/// Writes the Python SDK project into `out` and formats it with `ruff format`.
/// Returns every file written.
pub fn emit_sdk(api: &Api, opts: &Options, out: &Path) -> Result<Vec<PathBuf>, Error> {
    let env = env()?;
    let render = |name: &str, ctx: minijinja::Value| -> Result<String, Error> {
        Ok(env.get_template(name)?.render(ctx)?)
    };
    let c = sdk::common(api, opts);
    let src = format!("src/{}", opts.package);
    let mut files = vec![
        (
            "pyproject.toml".to_string(),
            render("sdk/pyproject.toml", context!(c))?,
        ),
        ("README.md".into(), render("sdk/README.md", context!(c))?),
        (
            format!("{src}/__init__.py"),
            render("sdk/__init__.py", context!(c))?,
        ),
        (
            format!("{src}/_version.py"),
            render("sdk/_version.py", context!(c))?,
        ),
        (
            format!("{src}/_client.py"),
            render("sdk/_client.py", context!(c))?,
        ),
        (
            format!("{src}/_base.py"),
            render("sdk/_base.py", context!(c))?,
        ),
        (
            format!("{src}/_auth.py"),
            render("sdk/_auth.py", context!(c))?,
        ),
        (
            format!("{src}/_errors.py"),
            render("sdk/_errors.py", context!(c))?,
        ),
        (
            format!("{src}/_utils.py"),
            render("sdk/_utils.py", context!(c))?,
        ),
        (
            format!("{src}/_pagination.py"),
            render("sdk/_pagination.py", context!(c))?,
        ),
        (
            format!("{src}/_streaming.py"),
            render("sdk/_streaming.py", context!(c))?,
        ),
        (
            format!("{src}/_response.py"),
            render("sdk/_response.py", context!(c))?,
        ),
        (format!("{src}/py.typed"), String::new()),
        (
            format!("{src}/types.py"),
            render("sdk/types.py", context!(t => sdk::types_ctx(api)))?,
        ),
        (
            format!("{src}/resources/__init__.py"),
            render("sdk/resources/__init__.py", context!(c))?,
        ),
    ];
    for r in &api.resources {
        files.push((
            format!("{src}/resources/{}.py", r.name),
            render(
                "sdk/resources/resource.py",
                context!(c, r => sdk::resource_ctx(api, r)),
            )?,
        ));
    }
    if c.has_webhooks {
        files.push((
            format!("{src}/resources/webhooks.py"),
            render("sdk/resources/webhooks.py", context!(c))?,
        ));
    }
    write_all(out, files)
}

/// Writes the Python MCP server project into `out` and formats it with `ruff format`.
/// Returns every file written. The server imports the SDK emitted by [`emit_sdk`].
pub fn emit_mcp(api: &Api, opts: &Options, out: &Path) -> Result<Vec<PathBuf>, Error> {
    let env = env()?;
    let c = mcp::ctx(api, opts);
    let render =
        |name: &str| -> Result<String, Error> { Ok(env.get_template(name)?.render(context!(c))?) };
    let src = format!("src/{}", c.mcp_pkg);
    let files = vec![
        ("pyproject.toml".to_string(), render("mcp/pyproject.toml")?),
        ("README.md".into(), render("mcp/README.md")?),
        (format!("{src}/__init__.py"), render("mcp/__init__.py")?),
        (format!("{src}/server.py"), render("mcp/server.py")?),
        (format!("{src}/py.typed"), String::new()),
    ];
    write_all(out, files)
}

fn write_all(out: &Path, files: Vec<(String, String)>) -> Result<Vec<PathBuf>, Error> {
    let mut paths = Vec::new();
    for (rel, content) in files {
        let path = out.join(rel);
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(&path, content)?;
        paths.push(path);
    }
    let python: Vec<&PathBuf> = paths
        .iter()
        .filter(|p| p.extension().is_some_and(|e| e == "py"))
        .collect();
    // Templates list the imports they might need; ruff sorts them and drops the unused ones.
    for args in [
        &[
            "check",
            "--no-cache",
            "--quiet",
            "--select",
            "I,F401",
            "--fix",
        ][..],
        &["format", "--no-cache"][..],
    ] {
        let output = Command::new("ruff")
            .args(args)
            .args(&python)
            .output()
            .map_err(|e| match e.kind() {
                std::io::ErrorKind::NotFound => Error::RuffMissing,
                _ => Error::Io(e),
            })?;
        if !output.status.success() {
            return Err(Error::Ruff(format!(
                "{}{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            )));
        }
    }
    Ok(paths)
}
