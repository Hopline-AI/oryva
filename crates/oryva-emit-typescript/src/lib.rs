//! Emits a TypeScript SDK (native `fetch`, no runtime dependencies) from an [`oryva_ir::Api`].
//! Names, signatures and doc comments are computed in Rust; the templates only lay them out.
//! Every `.ts` file is formatted in-process with `dprint-plugin-typescript`.

mod sdk;
mod ts;

use std::fmt;
use std::path::{Path, PathBuf};

use dprint_plugin_typescript::{
    FormatTextOptions, configuration::ConfigurationBuilder, format_text,
};
use minijinja::{Environment, context};
use oryva_ir::{Api, Auth};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Options {
    /// npm package name, e.g. `ledger`.
    pub package: String,
    /// Semver. Sent in the `User-Agent` header as `<name>-typescript/<version>`.
    pub version: String,
}

impl Options {
    pub fn for_api(api: &Api) -> Self {
        Options {
            package: api.name.replace('_', "-"),
            version: semver(&api.version),
        }
    }
}

/// npm needs semver: a date version `2026-10-01` becomes `2026.10.1`, anything else that isn't
/// `x.y.z` becomes `0.1.0`.
fn semver(version: &str) -> String {
    let numbers =
        |sep: char| -> Option<Vec<u64>> { version.split(sep).map(|p| p.parse().ok()).collect() };
    match (numbers('.'), numbers('-')) {
        (Some(n), _) if n.len() == 3 => version.to_string(),
        (_, Some(n)) if n.len() == 3 => format!("{}.{}.{}", n[0], n[1], n[2]),
        _ => "0.1.0".to_string(),
    }
}

#[derive(Debug)]
pub enum Error {
    Io(std::io::Error),
    Template(minijinja::Error),
    /// The formatter rejected a generated file, which means the generator wrote invalid
    /// TypeScript. Carries the file and the formatter's message.
    Format {
        file: String,
        message: String,
    },
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Io(e) => write!(f, "writing output: {e}"),
            Error::Template(e) => write!(f, "rendering template: {e:#}"),
            Error::Format { file, message } => {
                write!(f, "generated {file} is not valid TypeScript:\n{message}")
            }
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
    "runtime/core.ts",
    "runtime/errors.ts",
    "runtime/log.ts",
    "runtime/oauth.ts",
    "runtime/pagination.ts",
    "runtime/streaming.ts",
    "runtime/uploads.ts",
    "sdk/README.md",
    "sdk/client.ts",
    "sdk/gitignore",
    "sdk/index.ts",
    "sdk/package.json",
    "sdk/resource.ts",
    "sdk/tsconfig.json",
    "sdk/types.ts",
    "sdk/version.ts",
    "sdk/webhooks.ts",
];

fn env() -> Result<Environment<'static>, Error> {
    let mut env = Environment::new();
    env.set_trim_blocks(true);
    env.set_lstrip_blocks(true);
    env.set_keep_trailing_newline(true);
    env.add_filter("js", |s: String| ts::string(&s));
    env.add_filter("json", |s: String| {
        serde_json::to_string(&s).unwrap_or_default()
    });
    for (name, src) in TEMPLATES {
        env.add_template(name, src)?;
    }
    Ok(env)
}

/// Writes the TypeScript SDK project into `out` and formats it. Returns every file written.
pub fn emit_sdk(api: &Api, opts: &Options, out: &Path) -> Result<Vec<PathBuf>, Error> {
    let env = env()?;
    let render = |name: &str, ctx: minijinja::Value| -> Result<String, Error> {
        Ok(env.get_template(name)?.render(ctx)?)
    };
    let c = sdk::common(api, opts);
    let has_oauth = api
        .auth
        .iter()
        .any(|a| matches!(a, Auth::OAuth2ClientCredentials { .. }));

    let mut files: Vec<(String, String)> = vec![
        (
            "package.json".into(),
            render("sdk/package.json", context!(c))?,
        ),
        (
            "tsconfig.json".into(),
            render("sdk/tsconfig.json", context!(c))?,
        ),
        (".gitignore".into(), render("sdk/gitignore", context!(c))?),
        ("README.md".into(), render("sdk/README.md", context!(c))?),
        ("src/index.ts".into(), render("sdk/index.ts", context!(c))?),
        (
            "src/client.ts".into(),
            render("sdk/client.ts", context!(c))?,
        ),
        (
            "src/version.ts".into(),
            render("sdk/version.ts", context!(c))?,
        ),
    ];
    for name in [
        "core",
        "errors",
        "log",
        "pagination",
        "streaming",
        "uploads",
    ] {
        files.push((
            format!("src/{name}.ts"),
            render(&format!("runtime/{name}.ts"), context!(c))?,
        ));
    }
    if has_oauth {
        files.push((
            "src/oauth.ts".into(),
            render("runtime/oauth.ts", context!(c))?,
        ));
    }
    if let Some(secret_env) = &c.webhook_secret_env {
        let w = sdk::webhooks_ctx(api, secret_env);
        files.push((
            "src/webhooks.ts".into(),
            render("sdk/webhooks.ts", context!(c, w))?,
        ));
    }
    let source = sdk::types_source(api);
    files.push((
        "src/types.ts".into(),
        render(
            "sdk/types.ts",
            context!(uploadable => source.contains("Uploadable"), source),
        )?,
    ));
    for r in &api.resources {
        files.push((
            format!("src/resources/{}.ts", r.name),
            render("sdk/resource.ts", context!(r => sdk::resource_ctx(api, r)))?,
        ));
    }
    write_all(out, files)
}

fn write_all(out: &Path, files: Vec<(String, String)>) -> Result<Vec<PathBuf>, Error> {
    let mut paths = Vec::new();
    for (rel, mut content) in files {
        if rel.ends_with(".ts") {
            content = format_ts(&rel, &content)?;
        }
        let path = out.join(&rel);
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(&path, content)?;
        paths.push(path);
    }
    Ok(paths)
}

/// dprint returns `None` when the text is already formatted.
fn format_ts(file: &str, text: &str) -> Result<String, Error> {
    let config = ConfigurationBuilder::new()
        .line_width(100)
        .indent_width(2)
        .quote_style(dprint_plugin_typescript::configuration::QuoteStyle::PreferSingle)
        .build();
    let formatted = format_text(FormatTextOptions {
        path: Path::new(file),
        extension: None,
        text: text.to_string(),
        config: &config,
        external_formatter: None,
    })
    .map_err(|e| Error::Format {
        file: file.to_string(),
        message: e.to_string(),
    })?;
    Ok(formatted.unwrap_or_else(|| text.to_string()))
}
