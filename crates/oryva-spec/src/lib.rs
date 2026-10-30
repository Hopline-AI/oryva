//! OpenAPI 3.0/3.1 → [`oryva_ir::Api`].
//!
//! The spec is loaded into a `serde_json::Value` and walked directly, so both versions share
//! one code path. Only local `$ref`s into `#/components` are followed.

mod ops;
mod schema;
#[cfg(test)]
mod tests;

use heck::{ToShoutySnakeCase, ToSnakeCase};
use oryva_ir::{Api, Auth, IR_VERSION};
use schema::{Ctx, esc};
use serde_json::Value;
use std::path::Path;

#[derive(Debug, thiserror::Error)]
pub enum SpecError {
    #[error("cannot read {path}: {source}")]
    Read {
        path: String,
        source: std::io::Error,
    },
    #[error("cannot parse the spec as {format}: {message}")]
    Syntax {
        format: &'static str,
        message: String,
    },
    /// `at` is a JSON pointer into the spec, e.g. `#/paths/~1translate/post`.
    #[error("{at}: {message}")]
    Invalid { at: String, message: String },
}

pub(crate) fn invalid<T>(at: &str, message: impl Into<String>) -> Result<T, SpecError> {
    Err(SpecError::Invalid {
        at: at.into(),
        message: message.into(),
    })
}

pub fn parse_file(path: &Path) -> Result<Api, SpecError> {
    let src = std::fs::read_to_string(path).map_err(|source| SpecError::Read {
        path: path.display().to_string(),
        source,
    })?;
    parse_str(&src)
}

/// JSON if the first non-whitespace character is `{`, else YAML.
pub fn parse_str(src: &str) -> Result<Api, SpecError> {
    let doc: Value = if src.trim_start().starts_with('{') {
        serde_json::from_str(src).map_err(|e| SpecError::Syntax {
            format: "JSON",
            message: e.to_string(),
        })?
    } else {
        // Strict booleans: `no` (Norwegian) and `on` are strings in OpenAPI, not YAML 1.1 bools.
        let mut options = serde_saphyr::Options::default();
        options.strict_booleans = true;
        serde_saphyr::from_str_with_options(src, options).map_err(|e| SpecError::Syntax {
            format: "YAML",
            message: e.to_string(),
        })?
    };
    convert(&doc)
}

fn convert(doc: &Value) -> Result<Api, SpecError> {
    let version = doc.get("openapi").and_then(Value::as_str).unwrap_or("");
    if !(version.starts_with("3.0") || version.starts_with("3.1")) {
        return invalid(
            "#/openapi",
            format!(
                "expected OpenAPI 3.0 or 3.1, found {version:?}; convert Swagger 2.0 specs to 3.x first"
            ),
        );
    }
    let info = doc.get("info").unwrap_or(&Value::Null);
    let title = required_str(info, "title", "#/info/title")?;
    let api_version = required_str(info, "version", "#/info/version")?;
    let name = match info.get("x-oryva-name").and_then(Value::as_str) {
        Some(n) => n.to_snake_case(),
        None => title
            .split_whitespace()
            .next()
            .unwrap_or("")
            .to_snake_case(),
    };
    if name.is_empty() {
        return invalid(
            "#/info/title",
            "cannot derive a package name; set `info.x-oryva-name`",
        );
    }

    let mut cx = Ctx::new(doc);
    cx.components()?;
    let resources = cx.operations()?;
    Ok(Api {
        ir_version: IR_VERSION,
        auth: auth(doc, &name)?,
        title,
        version: api_version,
        description: info
            .get("description")
            .and_then(Value::as_str)
            .map(String::from),
        base_url: doc
            .pointer("/servers/0/url")
            .and_then(Value::as_str)
            .map(String::from),
        resources,
        types: cx.types.into_values().collect(),
        name,
    })
}

fn required_str(v: &Value, key: &str, at: &str) -> Result<String, SpecError> {
    match v.get(key).and_then(Value::as_str) {
        Some(s) => Ok(s.to_string()),
        None => invalid(at, format!("`{key}` is required and must be a string")),
    }
}

/// The scheme named by top-level `security`, else the only scheme defined.
fn auth(doc: &Value, name: &str) -> Result<Option<Auth>, SpecError> {
    let schemes = doc
        .pointer("/components/securitySchemes")
        .and_then(Value::as_object);
    let key = match doc.get("security").and_then(Value::as_array) {
        Some(reqs) => reqs
            .iter()
            .find_map(|r| r.as_object()?.keys().next().map(String::as_str)),
        None => schemes
            .filter(|s| s.len() == 1)
            .and_then(|s| s.keys().next().map(String::as_str)),
    };
    let Some(key) = key else { return Ok(None) };
    let at = format!("#/components/securitySchemes/{}", esc(key));
    let Some(scheme) = schemes.and_then(|s| s.get(key)) else {
        return invalid(&at, "`security` names a scheme that is not defined here");
    };
    let env_var = format!("{}_API_KEY", name.to_shouty_snake_case());
    let text = |k: &str| scheme.get(k).and_then(Value::as_str).unwrap_or("");
    match text("type") {
        "apiKey" if text("in") == "header" => Ok(Some(Auth::ApiKey {
            header: text("name").to_string(),
            env_var,
        })),
        "apiKey" => invalid(
            &at,
            "only apiKey schemes sent in a header are supported; the IR cannot express query or cookie keys",
        ),
        "http" if text("scheme").eq_ignore_ascii_case("bearer") => {
            Ok(Some(Auth::Bearer { env_var }))
        }
        "oauth2" | "openIdConnect" => Ok(Some(Auth::Bearer { env_var })),
        other => invalid(
            &at,
            format!(
                "unsupported security scheme ({other} {}); use apiKey in a header or http bearer",
                text("scheme")
            ),
        ),
    }
}
