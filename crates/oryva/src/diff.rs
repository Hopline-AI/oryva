//! `oryva diff`: what changed between two versions of an API, at the IR level.

use std::collections::BTreeSet;
use std::path::Path;

use oryva_ir::{
    Api, Auth, Body, Field, Method, Param, ParamLocation, ResponseBody, Shape, TypeDef, TypeRef,
};
use serde_json::{Value, json};

use crate::Result;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    Patch,
    Minor,
    Breaking,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Change {
    pub severity: Severity,
    pub text: String,
}

/// Loads a spec, or an IR file such as `.oryva/ir.json`.
pub fn load(path: &Path) -> Result<Api> {
    let src =
        std::fs::read_to_string(path).map_err(|e| format!("reading {}: {e}", path.display()))?;
    if src.trim_start().starts_with('{')
        && let Ok(v) = serde_json::from_str::<Value>(&src)
        && v.get("openapi").is_none()
        && let Some(version) = v.get("ir_version")
    {
        if version != oryva_ir::IR_VERSION {
            return Err(format!(
                "{} was written with IR version {version}, this oryva reads {}; diff against the spec instead",
                path.display(),
                oryva_ir::IR_VERSION
            )
            .into());
        }
        return Ok(serde_json::from_value(v)?);
    }
    Ok(oryva_spec::parse_str(&src)?)
}

pub fn diff(old: &Api, new: &Api) -> Vec<Change> {
    let mut c = Changes(Vec::new());
    c.auth(old, new);
    c.servers(old, new);
    c.resources(old, new);
    c.types(old, new);
    c.webhooks(old, new);
    c.0.sort_by(|a, b| b.severity.cmp(&a.severity));
    c.0
}

struct Changes(Vec<Change>);

impl Changes {
    fn add(&mut self, severity: Severity, text: String) {
        self.0.push(Change { severity, text });
    }

    fn breaking(&mut self, text: String) {
        self.add(Severity::Breaking, text);
    }

    fn minor(&mut self, text: String) {
        self.add(Severity::Minor, text);
    }

    fn patch(&mut self, text: String) {
        self.add(Severity::Patch, text);
    }

    fn text_changed(&mut self, at: &str, what: &str, old: &Option<String>, new: &Option<String>) {
        if old != new {
            self.patch(format!("{at}: {what} changed"));
        }
    }

    fn auth(&mut self, old: &Api, new: &Api) {
        let key = |a: &Auth| match a {
            Auth::ApiKey { location, name, .. } => format!("api key {name} in {location:?}"),
            Auth::Bearer { .. } => "bearer token".into(),
            Auth::Basic { .. } => "basic auth".into(),
            Auth::OAuth2ClientCredentials { .. } => "oauth2 client credentials".into(),
        };
        let (o, n): (BTreeSet<_>, BTreeSet<_>) = (
            old.auth.iter().map(key).collect(),
            new.auth.iter().map(key).collect(),
        );
        for k in o.difference(&n) {
            self.breaking(format!("auth: scheme {k} removed"));
        }
        for k in n.difference(&o) {
            self.minor(format!("auth: scheme {k} added"));
        }
    }

    fn servers(&mut self, old: &Api, new: &Api) {
        for s in &old.servers {
            if !new.servers.iter().any(|n| n.name == s.name) {
                self.breaking(format!("servers: environment {} removed", s.name));
            }
        }
        for s in &new.servers {
            if !old.servers.iter().any(|o| o.name == s.name) {
                self.minor(format!("servers: environment {} added", s.name));
            }
        }
    }

    fn webhooks(&mut self, old: &Api, new: &Api) {
        for w in &old.webhooks {
            match new.webhooks.iter().find(|n| n.name == w.name) {
                None => self.breaking(format!("webhooks: event {} removed", w.name)),
                Some(n) => self.ty(
                    &format!("webhooks.{}", w.name),
                    "payload",
                    &w.payload,
                    &n.payload,
                ),
            }
        }
        for w in &new.webhooks {
            if !old.webhooks.iter().any(|o| o.name == w.name) {
                self.minor(format!("webhooks: event {} added", w.name));
            }
        }
    }

    fn resources(&mut self, old: &Api, new: &Api) {
        for r in &old.resources {
            let Some(n) = new.resources.iter().find(|n| n.name == r.name) else {
                self.breaking(format!("{}: resource removed", r.name));
                continue;
            };
            self.text_changed(&r.name, "description", &r.description, &n.description);
            for m in &r.methods {
                let at = format!("{}.{}", r.name, m.name);
                match n.methods.iter().find(|x| x.name == m.name) {
                    None => self.breaking(format!("{at}: method removed")),
                    Some(nm) => self.method(&at, m, nm),
                }
            }
            for m in &n.methods {
                if !r.methods.iter().any(|x| x.name == m.name) {
                    self.minor(format!("{}.{}: method added", r.name, m.name));
                }
            }
        }
        for r in &new.resources {
            if !old.resources.iter().any(|o| o.name == r.name) {
                self.minor(format!("{}: resource added", r.name));
            }
        }
    }

    fn method(&mut self, at: &str, old: &Method, new: &Method) {
        if old.http_method != new.http_method {
            self.breaking(format!(
                "{at}: HTTP method changed from {:?} to {:?}",
                old.http_method, new.http_method
            ));
        }
        if old.path != new.path {
            self.breaking(format!(
                "{at}: path changed from {} to {}",
                old.path, new.path
            ));
        }
        self.text_changed(at, "summary", &old.summary, &new.summary);
        self.text_changed(at, "description", &old.description, &new.description);
        if !old.deprecated && new.deprecated {
            self.patch(format!("{at}: deprecated"));
        }
        if old.agent != new.agent {
            self.patch(format!("{at}: agent notes changed"));
        }
        self.params(at, &old.params, &new.params);
        self.body(at, old.body.as_ref(), new.body.as_ref());
        self.response(at, &old.response, &new.response);
    }

    fn params(&mut self, at: &str, old: &[Param], new: &[Param]) {
        let same = |a: &Param, b: &Param| a.name == b.name && a.location == b.location;
        for p in old {
            let Some(n) = new.iter().find(|n| same(p, n)) else {
                self.breaking(format!("{at}: param {} removed", p.name));
                continue;
            };
            if !p.required && n.required {
                self.breaking(format!("{at}: param {} is now required", p.name));
            }
            self.ty(at, &format!("param {}", p.name), &p.ty, &n.ty);
            self.text_changed(
                at,
                &format!("param {} description", p.name),
                &p.description,
                &n.description,
            );
            if p.default != n.default {
                self.patch(format!("{at}: param {} default changed", p.name));
            }
        }
        for n in new {
            if old.iter().any(|p| same(p, n)) {
                continue;
            }
            if n.required && n.location != ParamLocation::Header {
                self.breaking(format!("{at}: required param {} added", n.name));
            } else {
                self.minor(format!("{at}: param {} added", n.name));
            }
        }
    }

    fn body(&mut self, at: &str, old: Option<&Body>, new: Option<&Body>) {
        match (old, new) {
            (None, None) => {}
            (Some(_), None) => self.breaking(format!("{at}: request body removed")),
            (None, Some(n)) if n.required => {
                self.breaking(format!("{at}: required request body added"))
            }
            (None, Some(_)) => self.minor(format!("{at}: request body added")),
            (Some(o), Some(n)) => {
                if o.encoding != n.encoding {
                    self.breaking(format!(
                        "{at}: body encoding changed from {:?} to {:?}",
                        o.encoding, n.encoding
                    ));
                }
                if !o.required && n.required {
                    self.breaking(format!("{at}: request body is now required"));
                }
                self.ty(at, "request body", &o.ty, &n.ty);
            }
        }
    }

    fn response(&mut self, at: &str, old: &ResponseBody, new: &ResponseBody) {
        match (old, new) {
            (ResponseBody::Json { ty: o }, ResponseBody::Json { ty: n })
            | (ResponseBody::EventStream { event: o }, ResponseBody::EventStream { event: n }) => {
                self.ty(at, "response", o, n)
            }
            _ if old != new => self.breaking(format!(
                "{at}: response type changed from {} to {}",
                show_response(old),
                show_response(new)
            )),
            _ => {}
        }
    }

    /// Compares two type references; `what` names the thing that has the type.
    fn ty(&mut self, at: &str, what: &str, old: &TypeRef, new: &TypeRef) {
        if old == new {
            return;
        }
        match (old, new) {
            (TypeRef::Enum { values: o }, TypeRef::Enum { values: n }) => {
                self.values(at, what, o, n)
            }
            (TypeRef::Array { items: o }, TypeRef::Array { items: n })
            | (TypeRef::Map { values: o }, TypeRef::Map { values: n })
            | (TypeRef::Nullable { inner: o }, TypeRef::Nullable { inner: n }) => {
                self.ty(at, what, o, n)
            }
            _ => self.breaking(format!(
                "{at}: {what} type changed from {} to {}",
                show(old),
                show(new)
            )),
        }
    }

    fn values(&mut self, at: &str, what: &str, old: &[Value], new: &[Value]) {
        for v in old.iter().filter(|v| !new.contains(v)) {
            self.breaking(format!("{at}: {what} enum value {v} removed"));
        }
        for v in new.iter().filter(|v| !old.contains(v)) {
            self.minor(format!("{at}: {what} enum value {v} added"));
        }
    }

    fn types(&mut self, old: &Api, new: &Api) {
        let inputs = input_types(new);
        for t in &old.types {
            match new.type_def(&t.name) {
                None => self.breaking(format!("{}: type removed", t.name)),
                Some(n) => self.type_def(t, n, inputs.contains(&t.name)),
            }
        }
        for t in &new.types {
            if old.type_def(&t.name).is_none() {
                self.minor(format!("{}: type added", t.name));
            }
        }
    }

    fn type_def(&mut self, old: &TypeDef, new: &TypeDef, is_input: bool) {
        let at = old.name.as_str();
        self.text_changed(at, "description", &old.description, &new.description);
        match (&old.shape, &new.shape) {
            (Shape::Object { fields: o }, Shape::Object { fields: n }) => {
                self.fields(at, o, n, is_input)
            }
            (Shape::Enum { values: o }, Shape::Enum { values: n }) => self.values(at, "type", o, n),
            (
                Shape::Union {
                    variants: o,
                    discriminator: od,
                },
                Shape::Union {
                    variants: n,
                    discriminator: nd,
                },
            ) => {
                for v in o.iter().filter(|v| !n.contains(v)) {
                    self.breaking(format!("{at}: union variant {} removed", show(v)));
                }
                for v in n.iter().filter(|v| !o.contains(v)) {
                    self.minor(format!("{at}: union variant {} added", show(v)));
                }
                if od != nd {
                    self.breaking(format!("{at}: discriminator changed"));
                }
            }
            (Shape::Alias { target: o }, Shape::Alias { target: n }) => self.ty(at, "alias", o, n),
            _ => self.breaking(format!(
                "{at}: type changed from {} to {}",
                kind(&old.shape),
                kind(&new.shape)
            )),
        }
    }

    fn fields(&mut self, at: &str, old: &[Field], new: &[Field], is_input: bool) {
        for f in old {
            let Some(n) = new.iter().find(|n| n.name == f.name) else {
                self.breaking(format!("{at}: field {} removed", f.name));
                continue;
            };
            if !f.required && n.required {
                self.breaking(format!("{at}: field {} is now required", f.name));
            }
            self.ty(at, &format!("field {}", f.name), &f.ty, &n.ty);
            self.text_changed(
                at,
                &format!("field {} description", f.name),
                &f.description,
                &n.description,
            );
            if f.default != n.default {
                self.patch(format!("{at}: field {} default changed", f.name));
            }
        }
        for n in new.iter().filter(|n| !old.iter().any(|f| f.name == n.name)) {
            if n.required && is_input {
                self.breaking(format!("{at}: required field {} added", n.name));
            } else {
                self.minor(format!("{at}: field {} added", n.name));
            }
        }
    }
}

/// Names of the types a caller sends: reachable from request params and bodies.
fn input_types(api: &Api) -> BTreeSet<String> {
    let mut seen = BTreeSet::new();
    let mut todo: Vec<&TypeRef> = Vec::new();
    for m in api.resources.iter().flat_map(|r| &r.methods) {
        todo.extend(m.params.iter().map(|p| &p.ty));
        todo.extend(m.body.iter().map(|b| &b.ty));
    }
    while let Some(t) = todo.pop() {
        match t {
            TypeRef::Named { name } => {
                if seen.insert(name.clone())
                    && let Some(def) = api.type_def(name)
                {
                    match &def.shape {
                        Shape::Object { fields } => todo.extend(fields.iter().map(|f| &f.ty)),
                        Shape::Union { variants, .. } => todo.extend(variants),
                        Shape::Alias { target } => todo.push(target),
                        Shape::Enum { .. } => {}
                    }
                }
            }
            TypeRef::Array { items } => todo.push(items),
            TypeRef::Map { values } => todo.push(values),
            TypeRef::Nullable { inner } => todo.push(inner),
            TypeRef::Union { variants } => todo.extend(variants),
            _ => {}
        }
    }
    seen
}

fn kind(s: &Shape) -> &'static str {
    match s {
        Shape::Object { .. } => "object",
        Shape::Enum { .. } => "enum",
        Shape::Union { .. } => "union",
        Shape::Alias { .. } => "alias",
    }
}

fn show(t: &TypeRef) -> String {
    match t {
        TypeRef::String { format: Some(f) } => format!("string ({f})"),
        TypeRef::String { format: None } => "string".into(),
        TypeRef::Integer => "integer".into(),
        TypeRef::Number => "number".into(),
        TypeRef::Boolean => "boolean".into(),
        TypeRef::File => "file".into(),
        TypeRef::Any => "any".into(),
        TypeRef::Array { items } => format!("array of {}", show(items)),
        TypeRef::Map { values } => format!("map of {}", show(values)),
        TypeRef::Enum { .. } => "enum".into(),
        TypeRef::Named { name } => name.clone(),
        TypeRef::Nullable { inner } => format!("{} or null", show(inner)),
        TypeRef::Union { variants } => variants.iter().map(show).collect::<Vec<_>>().join(" | "),
    }
}

fn show_response(r: &ResponseBody) -> String {
    match r {
        ResponseBody::Empty => "empty".into(),
        ResponseBody::Json { ty } => show(ty),
        ResponseBody::Binary { content_type } => format!("binary ({content_type})"),
        ResponseBody::Text => "text".into(),
        ResponseBody::EventStream { event } => format!("event stream of {}", show(event)),
    }
}

/// The highest severity present, if any.
pub fn top(changes: &[Change]) -> Option<Severity> {
    changes.iter().map(|c| c.severity).max()
}

/// The next semver version. While on 0.x a breaking change bumps the minor version.
pub fn next_version(current: &str, top: Option<Severity>) -> Result<String> {
    let parts: Vec<u64> = current
        .split('.')
        .map(str::parse)
        .collect::<std::result::Result<_, _>>()
        .map_err(|_| format!("`{current}` is not a MAJOR.MINOR.PATCH version"))?;
    let [major, minor, patch] = parts[..] else {
        return Err(format!("`{current}` is not a MAJOR.MINOR.PATCH version").into());
    };
    Ok(match top {
        None => current.to_string(),
        Some(Severity::Breaking) if major == 0 => format!("0.{}.0", minor + 1),
        Some(Severity::Breaking) => format!("{}.0.0", major + 1),
        Some(Severity::Minor) => format!("{major}.{}.0", minor + 1),
        Some(Severity::Patch) => format!("{major}.{minor}.{}", patch + 1),
    })
}

pub fn counts(changes: &[Change]) -> [usize; 3] {
    let n = |s| changes.iter().filter(|c| c.severity == s).count();
    [
        n(Severity::Breaking),
        n(Severity::Minor),
        n(Severity::Patch),
    ]
}

pub fn markdown(changes: &[Change], version: Option<(&str, &str)>) -> String {
    let mut out = String::from("# API changes\n");
    if changes.is_empty() {
        out += "\nNo changes.\n";
    }
    for (severity, title) in [
        (Severity::Breaking, "Breaking"),
        (Severity::Minor, "Minor"),
        (Severity::Patch, "Patch"),
    ] {
        let items: Vec<_> = changes.iter().filter(|c| c.severity == severity).collect();
        if items.is_empty() {
            continue;
        }
        out += &format!("\n## {title} ({})\n\n", items.len());
        for c in items {
            out += &format!("- {}\n", c.text);
        }
    }
    if let Some((current, next)) = version {
        out += &format!("\nNext version: {current} -> {next}\n");
    }
    out
}

pub fn json_report(changes: &[Change], version: Option<(&str, &str)>) -> Value {
    let list = |s| -> Vec<&str> {
        changes
            .iter()
            .filter(|c| c.severity == s)
            .map(|c| c.text.as_str())
            .collect()
    };
    let mut v = json!({
        "breaking": list(Severity::Breaking),
        "minor": list(Severity::Minor),
        "patch": list(Severity::Patch),
    });
    if let Some((current, next)) = version {
        v["current_version"] = current.into();
        v["next_version"] = next.into();
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    const BASE: &str = r##"
openapi: 3.1.0
info: {title: Shop, version: "1.0.0"}
paths:
  /items:
    get:
      operationId: items_list
      tags: [items]
      description: List items.
      parameters:
        - {name: email, in: query, schema: {type: string}}
        - {name: kind, in: query, schema: {type: string, enum: [a, b]}}
      responses:
        "200":
          description: ok
          content:
            application/json:
              schema: {$ref: "#/components/schemas/Item"}
    post:
      operationId: items_create
      tags: [items]
      requestBody:
        required: true
        content:
          application/json:
            schema: {$ref: "#/components/schemas/NewItem"}
      responses:
        "200":
          description: ok
          content:
            application/json:
              schema: {$ref: "#/components/schemas/Item"}
components:
  schemas:
    Item:
      type: object
      description: An item.
      required: [id]
      properties:
        id: {type: string}
        note: {type: string}
        state: {type: string, enum: [open, closed]}
    NewItem:
      type: object
      required: [name]
      properties:
        name: {type: string}
        tag: {type: string}
    Unused: {type: object, properties: {x: {type: string}}}
"##;

    fn changes(edit: impl Fn(&str) -> String) -> Vec<Change> {
        let old = oryva_spec::parse_str(BASE).unwrap();
        let new = oryva_spec::parse_str(&edit(BASE)).unwrap();
        diff(&old, &new)
    }

    fn one(edit: impl Fn(&str) -> String) -> (Severity, String) {
        let c = changes(edit);
        assert_eq!(c.len(), 1, "{c:#?}");
        (c[0].severity, c[0].text.clone())
    }

    fn replace(from: &'static str, to: &'static str) -> impl Fn(&str) -> String {
        move |s| {
            assert!(s.contains(from), "{from} not in base");
            s.replacen(from, to, 1)
        }
    }

    #[test]
    fn identical_specs_have_no_changes() {
        assert!(changes(|s| s.to_string()).is_empty());
    }

    #[test]
    fn removed_param_is_breaking() {
        let r = replace(
            "        - {name: email, in: query, schema: {type: string}}\n",
            "",
        );
        assert_eq!(
            one(r),
            (Severity::Breaking, "items.list: param email removed".into())
        );
    }

    #[test]
    fn removed_method_is_breaking() {
        let r = |s: &str| {
            let (head, rest) = s.split_once("    post:").unwrap();
            let tail = rest.split_once("components:").unwrap().1;
            format!("{head}components:{tail}")
        };
        let c = changes(r);
        assert!(c.contains(&Change {
            severity: Severity::Breaking,
            text: "items.create: method removed".into()
        }));
        assert!(c.iter().all(|c| c.severity != Severity::Patch));
    }

    #[test]
    fn removed_resource_is_breaking() {
        let r = replace(
            "tags: [items]\n      description",
            "tags: [things]\n      description",
        );
        let c = changes(r);
        assert!(
            c.iter().any(|c| c.text == "items.list: method removed"),
            "{c:#?}"
        );
        assert!(c.iter().any(|c| c.text == "things: resource added"));
    }

    #[test]
    fn removed_field_is_breaking() {
        let r = replace("        note: {type: string}\n", "");
        assert_eq!(
            one(r),
            (Severity::Breaking, "Item: field note removed".into())
        );
    }

    #[test]
    fn removed_type_is_breaking() {
        let r = replace(
            "    Unused: {type: object, properties: {x: {type: string}}}\n",
            "",
        );
        assert_eq!(one(r), (Severity::Breaking, "Unused: type removed".into()));
    }

    #[test]
    fn removed_enum_value_is_breaking() {
        let r = replace("enum: [open, closed]", "enum: [open]");
        let (s, t) = one(r);
        assert_eq!(s, Severity::Breaking);
        assert_eq!(t, "Item: field state enum value \"closed\" removed");
    }

    #[test]
    fn removed_auth_scheme_is_breaking() {
        let with_auth = |s: &str| {
            format!(
                "{s}  securitySchemes:\n    k: {{type: http, scheme: bearer}}\nsecurity:\n  - k: []\n"
            )
        };
        let old = oryva_spec::parse_str(&with_auth(BASE)).unwrap();
        let new = oryva_spec::parse_str(BASE).unwrap();
        let removed = diff(&old, &new);
        assert!(
            removed.iter().any(|c| c.severity == Severity::Breaking
                && c.text == "auth: scheme bearer token removed"),
            "{removed:#?}"
        );
        let added = diff(&new, &old);
        assert!(
            added
                .iter()
                .any(|c| c.severity == Severity::Minor
                    && c.text == "auth: scheme bearer token added")
        );
    }

    #[test]
    fn optional_made_required_is_breaking() {
        let r = replace(
            "        - {name: email, in: query, schema: {type: string}}",
            "        - {name: email, in: query, required: true, schema: {type: string}}",
        );
        assert_eq!(
            one(r),
            (
                Severity::Breaking,
                "items.list: param email is now required".into()
            )
        );
        let r = replace("required: [name]", "required: [name, tag]");
        assert_eq!(
            one(r),
            (
                Severity::Breaking,
                "NewItem: field tag is now required".into()
            )
        );
    }

    #[test]
    fn changed_type_is_breaking() {
        let r = replace(
            "email, in: query, schema: {type: string}",
            "email, in: query, schema: {type: integer}",
        );
        assert_eq!(
            one(r),
            (
                Severity::Breaking,
                "items.list: param email type changed from string to integer".into()
            )
        );
    }

    #[test]
    fn changed_response_type_is_breaking() {
        let r = |s: &str| {
            let (a, b) = s.split_once("operationId: items_create").unwrap();
            let b = b.replacen(
                "#/components/schemas/Item",
                "#/components/schemas/NewItem",
                1,
            );
            format!("{a}operationId: items_create{b}")
        };
        let c = changes(r);
        assert!(
            c.iter().any(|c| c.severity == Severity::Breaking
                && c.text == "items.create: response type changed from Item to NewItem"),
            "{c:#?}"
        );
    }

    #[test]
    fn changed_http_method_or_path_is_breaking() {
        let c = changes(replace("/items:", "/things:"));
        assert!(c.iter().any(|c| c.severity == Severity::Breaking
            && c.text.contains("path changed from /items to /things")));
        let r = replace(
            "    get:\n      operationId: items_list",
            "    put:\n      operationId: items_list",
        );
        let c = changes(r);
        assert!(c.iter().any(|c| c.severity == Severity::Breaking && c.text.contains("HTTP method changed")), "{c:#?}");
    }

    #[test]
    fn additions_are_minor() {
        let r = replace(
            "        - {name: email, in: query, schema: {type: string}}",
            "        - {name: email, in: query, schema: {type: string}}\n        - {name: extra, in: query, schema: {type: string}}",
        );
        assert_eq!(
            one(r),
            (Severity::Minor, "items.list: param extra added".into())
        );
        let r = replace(
            "        note: {type: string}",
            "        note: {type: string}\n        extra: {type: string}",
        );
        assert_eq!(one(r), (Severity::Minor, "Item: field extra added".into()));
        let r = replace("enum: [open, closed]", "enum: [open, closed, stale]");
        assert_eq!(one(r).0, Severity::Minor);
        let r = replace(
            "    Unused:",
            "    Fresh: {type: object, properties: {y: {type: string}}}\n    Unused:",
        );
        assert_eq!(one(r), (Severity::Minor, "Fresh: type added".into()));
        let r = |s: &str| {
            s.replace(
                "components:",
                "  /other:\n    get:\n      operationId: other_list\n      tags: [other]\n      responses: {\"200\": {description: ok}}\ncomponents:",
            )
        };
        let c = changes(r);
        assert!(
            c.iter()
                .any(|c| c.severity == Severity::Minor && c.text == "other: resource added")
        );
        let r = replace(
            "  /items:\n    get:",
            "  /items:\n    patch:\n      operationId: items_update\n      tags: [items]\n      responses: {\"200\": {description: ok}}\n    get:",
        );
        assert_eq!(
            one(r),
            (Severity::Minor, "items.update: method added".into())
        );
    }

    #[test]
    fn new_required_field_is_breaking_only_in_request_types() {
        let r = replace(
            "        tag: {type: string}",
            "        tag: {type: string}\n        must: {type: string}",
        );
        assert_eq!(one(r).0, Severity::Minor);
        let r = |s: &str| {
            s.replace("required: [name]", "required: [name, must]")
                .replace(
                    "        tag: {type: string}",
                    "        tag: {type: string}\n        must: {type: string}",
                )
        };
        assert_eq!(
            one(r),
            (
                Severity::Breaking,
                "NewItem: required field must added".into()
            )
        );
    }

    #[test]
    fn descriptions_are_patch() {
        assert_eq!(
            one(replace("List items.", "List all items.")),
            (Severity::Patch, "items.list: description changed".into())
        );
        assert_eq!(
            one(replace("An item.", "One item.")),
            (Severity::Patch, "Item: description changed".into())
        );
    }

    #[test]
    fn versions() {
        use Severity::*;
        assert_eq!(next_version("1.4.0", Some(Breaking)).unwrap(), "2.0.0");
        assert_eq!(next_version("0.3.1", Some(Breaking)).unwrap(), "0.4.0");
        assert_eq!(next_version("1.4.2", Some(Minor)).unwrap(), "1.5.0");
        assert_eq!(next_version("1.4.2", Some(Patch)).unwrap(), "1.4.3");
        assert_eq!(next_version("1.4.2", None).unwrap(), "1.4.2");
        assert!(next_version("2026-10-01", Some(Patch)).is_err());
    }
}
