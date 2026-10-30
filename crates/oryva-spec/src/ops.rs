//! Paths and operations → resources and methods.

use crate::SpecError;
use crate::invalid;
use crate::schema::{Ctx, esc, pascal, string_at};
use heck::ToSnakeCase;
use oryva_ir::{
    AgentNotes, Body, BodyEncoding, ErrorCase, HttpMethod, Method, Param, ParamLocation, Resource,
    ResponseBody, TypeRef,
};
use serde_json::{Map, Value};
use std::collections::HashMap;

const VERBS: [(&str, HttpMethod); 5] = [
    ("get", HttpMethod::Get),
    ("put", HttpMethod::Put),
    ("post", HttpMethod::Post),
    ("delete", HttpMethod::Delete),
    ("patch", HttpMethod::Patch),
];

fn json_key(content: &Map<String, Value>) -> Option<&String> {
    content
        .keys()
        .find(|k| k.starts_with("application/json") || k.ends_with("+json"))
}

fn ext_str<'v>(op: &'v Value, key: &str, at: &str) -> Result<Option<&'v str>, SpecError> {
    match op.get(key) {
        None => Ok(None),
        Some(Value::String(s)) => Ok(Some(s)),
        Some(_) => invalid(&format!("{at}/{key}"), "expected a string"),
    }
}

impl<'a> Ctx<'a> {
    pub fn operations(&mut self) -> Result<Vec<Resource>, SpecError> {
        let mut resources: Vec<Resource> = Vec::new();
        let mut taken: HashMap<(String, String), String> = HashMap::new();
        let Some(paths) = self.root.get("paths").and_then(Value::as_object) else {
            return Ok(resources);
        };
        for (path, item) in paths {
            let item_at = format!("#/paths/{}", esc(path));
            for (verb, op) in item.as_object().into_iter().flatten() {
                let Some(&(_, http)) = VERBS.iter().find(|(v, _)| v == verb) else {
                    continue;
                };
                let at = format!("{item_at}/{verb}");
                let resource = self.resource_name(path, op, &at)?;
                let name = method_name(path, op, http, &resource, &at)?;
                if let Some(first) = taken.insert((resource.clone(), name.clone()), at.clone()) {
                    return invalid(
                        &at,
                        format!(
                            "maps to `{resource}.{name}`, which {first} already uses; set `x-oryva-method` (or `x-oryva-resource`) on one of them"
                        ),
                    );
                }
                let hint = format!("{}{}", pascal(&resource), pascal(&name));
                let (response, errors) = self.responses(op, &hint, &at)?;
                let method = Method {
                    params: self.params(item, &item_at, op, &at, &hint)?,
                    body: self.body(op, &at, &hint)?,
                    name,
                    http_method: http,
                    path: path.clone(),
                    summary: string_at(op, "summary"),
                    description: string_at(op, "description"),
                    response,
                    errors,
                    deprecated: op
                        .get("deprecated")
                        .and_then(Value::as_bool)
                        .unwrap_or(false),
                    agent: agent(op, http, &at)?,
                };
                match resources.iter_mut().find(|r| r.name == resource) {
                    Some(r) => r.methods.push(method),
                    None => resources.push(Resource {
                        description: self.tag_description(&resource),
                        name: resource,
                        methods: vec![method],
                    }),
                }
            }
        }
        Ok(resources)
    }

    fn resource_name(&self, path: &str, op: &Value, at: &str) -> Result<String, SpecError> {
        let chosen = match ext_str(op, "x-oryva-resource", at)? {
            Some(r) => Some(r),
            None => op.pointer("/tags/0").and_then(Value::as_str).or_else(|| {
                path.split('/')
                    .find(|s| !s.is_empty() && !s.starts_with('{'))
            }),
        };
        match chosen.map(|r| r.to_snake_case()) {
            Some(r) if !r.is_empty() => Ok(r),
            _ => invalid(
                at,
                "cannot pick a resource name; add a tag or set `x-oryva-resource`",
            ),
        }
    }

    /// Description of the top-level tag that matches the resource name.
    fn tag_description(&self, resource: &str) -> Option<String> {
        self.root
            .get("tags")?
            .as_array()?
            .iter()
            .find(|t| string_at(t, "name").is_some_and(|n| n.to_snake_case() == resource))
            .and_then(|t| string_at(t, "description"))
    }

    /// Path-item parameters first, then the operation's; the operation wins on name + location.
    fn params(
        &mut self,
        item: &'a Value,
        item_at: &str,
        op: &'a Value,
        op_at: &str,
        hint: &str,
    ) -> Result<Vec<Param>, SpecError> {
        let mut raw: Vec<(String, String, &'a Value, String)> = Vec::new();
        for (owner, owner_at) in [(item, item_at), (op, op_at)] {
            for (i, p) in owner
                .get("parameters")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .enumerate()
            {
                let (p, at) =
                    self.resolve(p, &format!("{owner_at}/parameters/{i}"), "parameters")?;
                let (Some(name), Some(loc)) = (string_at(p, "name"), string_at(p, "in")) else {
                    return invalid(&at, "a parameter needs string `name` and `in`");
                };
                if loc == "cookie" {
                    continue;
                }
                let entry = (name, loc, p, at);
                match raw.iter_mut().find(|r| r.0 == entry.0 && r.1 == entry.1) {
                    Some(slot) => *slot = entry,
                    None => raw.push(entry),
                }
            }
        }
        let mut out = Vec::new();
        for (name, loc, p, at) in raw {
            let location = match loc.as_str() {
                "path" => ParamLocation::Path,
                "query" => ParamLocation::Query,
                "header" => ParamLocation::Header,
                other => return invalid(&at, format!("unknown parameter location {other:?}")),
            };
            let ty = match p.get("schema") {
                Some(s) => self.ty(
                    s,
                    &format!("{hint}{}", pascal(&name)),
                    &format!("{at}/schema"),
                )?,
                None => TypeRef::Any,
            };
            out.push(Param {
                ident: name.to_snake_case(),
                required: location == ParamLocation::Path
                    || p.get("required").and_then(Value::as_bool).unwrap_or(false),
                description: string_at(p, "description"),
                location,
                ty,
                name,
            });
        }
        Ok(out)
    }

    fn body(&mut self, op: &'a Value, at: &str, hint: &str) -> Result<Option<Body>, SpecError> {
        let Some(rb) = op.get("requestBody") else {
            return Ok(None);
        };
        let (rb, rb_at) = self.resolve(rb, &format!("{at}/requestBody"), "requestBodies")?;
        let content = rb.get("content").and_then(Value::as_object);
        let found = content.and_then(|c| {
            json_key(c)
                .map(|k| (k, BodyEncoding::Json))
                .or_else(|| {
                    c.keys()
                        .find(|k| *k == "multipart/form-data")
                        .map(|k| (k, BodyEncoding::Multipart))
                })
                .or_else(|| {
                    c.keys()
                        .find(|k| *k == "application/x-www-form-urlencoded")
                        .map(|k| (k, BodyEncoding::Form))
                })
        });
        let (Some(content), Some((key, encoding))) = (content, found) else {
            let offered: Vec<&str> = content
                .into_iter()
                .flat_map(|c| c.keys().map(String::as_str))
                .collect();
            return invalid(
                &rb_at,
                format!(
                    "request body must offer application/json, multipart/form-data or application/x-www-form-urlencoded; found {offered:?}"
                ),
            );
        };
        let ty = match content[key].get("schema") {
            Some(s) => self.ty(
                s,
                &format!("{hint}Request"),
                &format!("{rb_at}/content/{}/schema", esc(key)),
            )?,
            None => TypeRef::Any,
        };
        Ok(Some(Body {
            encoding,
            required: rb.get("required").and_then(Value::as_bool).unwrap_or(false),
            ty,
        }))
    }

    /// The first 2xx response becomes the return value; every 4xx/5xx becomes an `ErrorCase`.
    fn responses(
        &mut self,
        op: &'a Value,
        hint: &str,
        at: &str,
    ) -> Result<(ResponseBody, Vec<ErrorCase>), SpecError> {
        let Some(map) = op.get("responses").and_then(Value::as_object) else {
            return Ok((ResponseBody::Empty, vec![]));
        };
        let mut codes: Vec<(u16, &String, &'a Value)> = map
            .iter()
            .filter_map(|(k, v)| Some((k.parse().ok()?, k, v)))
            .collect();
        codes.sort_by_key(|c| c.0);

        let mut response = ResponseBody::Empty;
        if let Some(&(_, key, v)) = codes.iter().find(|c| (200..300).contains(&c.0)) {
            let (r, r_at) = self.resolve(v, &format!("{at}/responses/{key}"), "responses")?;
            if let Some(content) = r.get("content").and_then(Value::as_object) {
                if let Some(k) = json_key(content) {
                    let ty = self.media_type(&content[k], &r_at, k, &format!("{hint}Response"))?;
                    response = ResponseBody::Json { ty };
                } else if content.keys().any(|k| k.starts_with("text/")) {
                    response = ResponseBody::Text;
                } else if let Some(k) = content.keys().next() {
                    response = ResponseBody::Binary {
                        content_type: k.split(';').next().unwrap_or(k).trim().to_string(),
                    };
                }
            }
        }

        let mut errors = Vec::new();
        for &(status, key, v) in codes.iter().filter(|c| (400..600).contains(&c.0)) {
            let (r, r_at) = self.resolve(v, &format!("{at}/responses/{key}"), "responses")?;
            let mut ty = None;
            if let Some(content) = r.get("content").and_then(Value::as_object)
                && let Some(k) = json_key(content)
            {
                ty = Some(self.media_type(
                    &content[k],
                    &r_at,
                    k,
                    &format!("{hint}Error{status}"),
                )?);
            }
            errors.push(ErrorCase {
                status,
                description: string_at(r, "description"),
                ty,
            });
        }
        Ok((response, errors))
    }

    fn media_type(
        &mut self,
        media: &'a Value,
        at: &str,
        key: &str,
        hint: &str,
    ) -> Result<TypeRef, SpecError> {
        match media.get("schema") {
            Some(s) => self.ty(s, hint, &format!("{at}/content/{}/schema", esc(key))),
            None => Ok(TypeRef::Any),
        }
    }
}

fn method_name(
    path: &str,
    op: &Value,
    http: HttpMethod,
    resource: &str,
    at: &str,
) -> Result<String, SpecError> {
    if let Some(m) = ext_str(op, "x-oryva-method", at)? {
        return Ok(m.to_snake_case());
    }
    if let Some(id) = ext_str(op, "operationId", at)? {
        let id = id.to_snake_case();
        return Ok(match id.strip_prefix(&format!("{resource}_")) {
            Some(rest) if !rest.is_empty() => rest.to_string(),
            _ => id,
        });
    }
    let by_id = path.trim_end_matches('/').ends_with('}');
    Ok(match http {
        HttpMethod::Get if by_id => "get",
        HttpMethod::Get => "list",
        HttpMethod::Post => "create",
        HttpMethod::Put | HttpMethod::Patch => "update",
        HttpMethod::Delete => "delete",
    }
    .into())
}

/// Defaults from the HTTP method, then `x-oryva-agent` keys override one by one.
fn agent(op: &Value, http: HttpMethod, at: &str) -> Result<AgentNotes, SpecError> {
    let mut notes = AgentNotes {
        read_only: http == HttpMethod::Get,
        destructive: http == HttpMethod::Delete,
        idempotent: matches!(http, HttpMethod::Get | HttpMethod::Put | HttpMethod::Delete),
        ..Default::default()
    };
    let Some(ext) = op.get("x-oryva-agent") else {
        return Ok(notes);
    };
    let ext_at = format!("{at}/x-oryva-agent");
    let Some(ext) = ext.as_object() else {
        return invalid(&ext_at, "expected a mapping");
    };
    for (key, v) in ext {
        let at = format!("{ext_at}/{key}");
        let flag = || match v.as_bool() {
            Some(b) => Ok(b),
            None => invalid(&at, "expected true or false"),
        };
        match key.as_str() {
            "read_only" => notes.read_only = flag()?,
            "destructive" => notes.destructive = flag()?,
            "idempotent" => notes.idempotent = flag()?,
            "hint" => match v.as_str() {
                Some(s) => notes.hint = Some(s.to_string()),
                None => return invalid(&at, "expected a string"),
            },
            "examples" => match v.as_array() {
                Some(a) => notes.examples = a.clone(),
                None => return invalid(&at, "expected a list"),
            },
            _ => {
                return invalid(
                    &at,
                    "unknown key; expected read_only, destructive, idempotent, hint or examples",
                );
            }
        }
    }
    Ok(notes)
}
