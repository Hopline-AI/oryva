//! OpenAPI 3.0/3.1 → [`oryva_ir::Api`].
//!
//! The spec is loaded into a `serde_json::Value` and walked directly, so both versions share
//! one code path. Only local `$ref`s into `#/components` are followed.

mod ops;
mod schema;
#[cfg(test)]
mod tests;

use heck::{ToShoutySnakeCase, ToSnakeCase};
use oryva_ir::{Api, ApiKeyLocation, Auth, IR_VERSION, Server, WebhookSigning};
use schema::{Ctx, esc, string_at};
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
        // Specs are the user's own input; real ones run to 11 MB, past the default budget.
        options.budget = None;
        serde_saphyr::from_str_with_options(src, options).map_err(|e| SpecError::Syntax {
            format: "YAML",
            message: e.to_string(),
        })?
    };
    convert(&doc)
}

fn convert(doc: &Value) -> Result<Api, SpecError> {
    let version = doc.get("openapi").and_then(Value::as_str).unwrap_or("");
    if let Some(v) = doc.get("swagger").and_then(Value::as_str) {
        return invalid(
            "#/swagger",
            format!(
                "Swagger {v} is not supported, only OpenAPI 3.0 and 3.1; convert it first with `npx swagger2openapi spec.yaml -o openapi.yaml`"
            ),
        );
    }
    // 3.2 is 3.1 plus additions that no emitter reads, so it shares the 3.1 path.
    if !["3.0", "3.1", "3.2"].iter().any(|v| version.starts_with(v)) {
        return invalid(
            "#/openapi",
            format!("expected OpenAPI 3.0, 3.1 or 3.2, found {version:?}"),
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
    let webhooks = cx.webhooks()?;
    let upper = name.to_shouty_snake_case();
    let idempotency_header = match info.get("x-oryva-idempotency-header") {
        None => None,
        Some(Value::String(h)) => Some(h.clone()),
        Some(_) => return invalid("#/info/x-oryva-idempotency-header", "expected a string"),
    };
    let webhook_signing = match info.get("x-oryva-webhook-signing") {
        None => None,
        Some(Value::String(s)) if s == "standard-webhooks" => {
            Some(WebhookSigning::StandardWebhooks {
                secret_env: format!("{upper}_WEBHOOK_SECRET"),
            })
        }
        Some(other) => {
            return invalid(
                "#/info/x-oryva-webhook-signing",
                format!("unsupported value {other}; expected `standard-webhooks`"),
            );
        }
    };
    Ok(Api {
        ir_version: IR_VERSION,
        auth: auth(doc, &name)?,
        title,
        version: api_version,
        description: info
            .get("description")
            .and_then(Value::as_str)
            .map(String::from),
        servers: servers(doc)?,
        idempotency_header,
        webhooks,
        webhook_signing,
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

/// Every `servers` entry, named by `x-oryva-name`, else the first label of its host, else
/// `environment_<n>` (also on a name collision). Variables are replaced by their defaults.
fn servers(doc: &Value) -> Result<Vec<Server>, SpecError> {
    let mut out: Vec<Server> = Vec::new();
    for (i, s) in doc
        .get("servers")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .enumerate()
    {
        let at = format!("#/servers/{i}");
        let mut url = required_str(s, "url", &format!("{at}/url"))?;
        for (var, def) in s
            .get("variables")
            .and_then(Value::as_object)
            .into_iter()
            .flatten()
        {
            if let Some(d) = string_at(def, "default") {
                url = url.replace(&format!("{{{var}}}"), &d);
            }
        }
        let named = match s.get("x-oryva-name") {
            None => None,
            Some(Value::String(n)) => Some(n.to_snake_case()),
            Some(_) => return invalid(&format!("{at}/x-oryva-name"), "expected a string"),
        };
        let from_host = || {
            let host = url.split_once("://").map_or("", |(_, rest)| rest);
            let host = host.split(['/', '?', '#', ':']).next().unwrap_or("");
            host.split('.').next().unwrap_or("").to_snake_case()
        };
        let mut name = named.unwrap_or_else(from_host);
        if name.is_empty()
            || name.starts_with(|c: char| c.is_ascii_digit())
            || out.iter().any(|o| o.name == name)
        {
            name = format!("environment_{}", i + 1);
        }
        out.push(Server {
            name,
            url,
            description: string_at(s, "description"),
        });
    }
    Ok(out)
}

/// Every scheme named by top-level `security`, in order, else the only scheme defined. Each
/// requirement is an alternative; one that combines schemes is an error.
fn auth(doc: &Value, name: &str) -> Result<Vec<Auth>, SpecError> {
    let schemes = doc
        .pointer("/components/securitySchemes")
        .and_then(Value::as_object);
    let mut wanted: Vec<(&str, Vec<String>)> = Vec::new();
    match doc.get("security").and_then(Value::as_array) {
        Some(reqs) => {
            for (i, req) in reqs.iter().enumerate() {
                let at = format!("#/security/{i}");
                let Some(req) = req.as_object() else {
                    return invalid(&at, "expected a mapping of scheme name to scopes");
                };
                if req.len() > 1 {
                    let keys: Vec<&String> = req.keys().collect();
                    return invalid(
                        &at,
                        format!(
                            "requirements that combine schemes ({keys:?}) are not supported, since credentials that must be sent together cannot be modelled yet; edit `security` so each scheme is its own alternative (the generated client sends the first one it has credentials for)"
                        ),
                    );
                }
                if let Some((key, scopes)) = req.iter().next()
                    && !wanted.iter().any(|(k, _)| k == key)
                {
                    let scopes = scopes.as_array().into_iter().flatten();
                    let scopes = scopes.filter_map(|v| v.as_str().map(String::from));
                    wanted.push((key, scopes.collect()));
                }
            }
        }
        None => {
            if let Some(only) = schemes
                .filter(|s| s.len() == 1)
                .and_then(|s| s.keys().next())
            {
                wanted.push((only, vec![]));
            }
        }
    }
    let upper = name.to_shouty_snake_case();
    let mut out = Vec::new();
    for (key, scopes) in wanted {
        let at = format!("#/components/securitySchemes/{}", esc(key));
        let Some(scheme) = schemes.and_then(|s| s.get(key)) else {
            return invalid(&at, "`security` names a scheme that is not defined here");
        };
        out.push(scheme_auth(scheme, scopes, &upper, &at)?);
    }
    Ok(out)
}

fn scheme_auth(
    scheme: &Value,
    scopes: Vec<String>,
    upper: &str,
    at: &str,
) -> Result<Auth, SpecError> {
    let text = |k: &str| scheme.get(k).and_then(Value::as_str).unwrap_or("");
    let env_var = format!("{upper}_API_KEY");
    match text("type") {
        "apiKey" => {
            let location = match text("in") {
                "header" => ApiKeyLocation::Header,
                "query" => ApiKeyLocation::Query,
                "cookie" => ApiKeyLocation::Cookie,
                other => {
                    return invalid(
                        at,
                        format!("apiKey `in` must be header, query or cookie, found {other:?}"),
                    );
                }
            };
            Ok(Auth::ApiKey {
                location,
                name: text("name").to_string(),
                env_var,
            })
        }
        "http" if text("scheme").eq_ignore_ascii_case("bearer") => Ok(Auth::Bearer { env_var }),
        "http" if text("scheme").eq_ignore_ascii_case("basic") => Ok(Auth::Basic {
            username_env: format!("{upper}_USERNAME"),
            password_env: format!("{upper}_PASSWORD"),
        }),
        "oauth2" => match scheme.pointer("/flows/clientCredentials") {
            Some(flow) => {
                let flow_at = format!("{at}/flows/clientCredentials");
                let Some(token_url) = string_at(flow, "tokenUrl") else {
                    return invalid(&format!("{flow_at}/tokenUrl"), "`tokenUrl` is required");
                };
                let scopes = if scopes.is_empty() {
                    let all = flow.get("scopes").and_then(Value::as_object);
                    all.into_iter().flat_map(|m| m.keys().cloned()).collect()
                } else {
                    scopes
                };
                Ok(Auth::OAuth2ClientCredentials {
                    token_url,
                    scopes,
                    client_id_env: format!("{upper}_CLIENT_ID"),
                    client_secret_env: format!("{upper}_CLIENT_SECRET"),
                })
            }
            // The caller brings a token.
            None => Ok(Auth::Bearer { env_var }),
        },
        "openIdConnect" => Ok(Auth::Bearer { env_var }),
        other => invalid(
            at,
            format!(
                "unsupported security scheme ({other} {}); use apiKey, http bearer or basic, oauth2 or openIdConnect",
                text("scheme")
            ),
        ),
    }
}
