//! Python-flavoured views of the IR shared by the SDK and MCP emitters.

use std::collections::{BTreeMap, BTreeSet};

use oryva_ir::{Api, BodyEncoding, Method, ParamLocation, ResponseBody, Shape, TypeRef};
use serde_json::Value;

/// Names a rendered snippet needs imported.
#[derive(Default)]
pub struct Uses {
    /// Names from `typing`.
    pub typing: BTreeSet<&'static str>,
    /// Names from `datetime`.
    pub datetime: BTreeSet<&'static str>,
    /// Generated type names (from `types.py`).
    pub named: BTreeSet<String>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Target {
    Sdk,
    Mcp,
}

pub fn pascal(snake: &str) -> String {
    snake
        .split('_')
        .filter(|p| !p.is_empty())
        .map(|p| {
            let mut c = p.chars();
            c.next()
                .map(|f| f.to_uppercase().chain(c).collect::<String>())
                .unwrap_or_default()
        })
        .collect()
}

/// Python's hard keywords (`keyword.kwlist`).
const KEYWORDS: &[&str] = &[
    "False", "None", "True", "and", "as", "assert", "async", "await", "break", "class", "continue",
    "def", "del", "elif", "else", "except", "finally", "for", "from", "global", "if", "import",
    "in", "is", "lambda", "nonlocal", "not", "or", "pass", "raise", "return", "try", "while",
    "with", "yield",
];

/// A valid Python identifier for a name from the spec: invalid characters become `_`, a leading
/// number gets a `_` in front and a keyword gets a trailing `_` (`from` -> `from_`).
pub fn ident(name: &str) -> String {
    let mut out: String = name
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if out.is_empty() || out.starts_with(|c: char| c.is_ascii_digit()) {
        out.insert(0, '_');
    }
    if KEYWORDS.contains(&out.as_str()) {
        out.push('_');
    }
    out
}

/// Members of pydantic's `BaseModel`; a field with one of these names would override it.
const MODEL_MEMBERS: &[&str] = &[
    "construct",
    "copy",
    "dict",
    "from_orm",
    "json",
    "parse_file",
    "parse_obj",
    "parse_raw",
    "schema",
    "schema_json",
    "update_forward_refs",
    "validate",
    "model_computed_fields",
    "model_config",
    "model_construct",
    "model_copy",
    "model_dump",
    "model_dump_json",
    "model_extra",
    "model_fields",
    "model_fields_set",
    "model_json_schema",
    "model_parametrized_name",
    "model_post_init",
    "model_rebuild",
    "model_validate",
    "model_validate_json",
    "model_validate_strings",
];

/// The attribute name of a model field. The wire name stays reachable through an alias.
pub fn field_ident(ident: &str) -> String {
    // pydantic rejects leading underscores in field names (`+1` snake-cases to `_1`).
    if ident.starts_with('_') {
        format!("field{ident}")
    } else if MODEL_MEMBERS.contains(&ident) {
        format!("{ident}_")
    } else {
        ident.to_string()
    }
}

/// The class for a resource. A component schema of the same name wins the plain name.
pub fn resource_class(api: &Api, name: &str) -> String {
    let class = pascal(name);
    // `Environment` and the client class are defined next to the resource imports.
    if api.type_def(&class).is_some() || class == "Environment" || class == pascal(&api.name) {
        format!("{class}Resource")
    } else {
        class
    }
}

/// Names the generated modules import (or define), so a model must not take them.
const RESERVED_TYPES: &[&str] = &[
    "Annotated",
    "Any",
    "APIResponse",
    "AsyncPage",
    "AsyncStream",
    "BaseModel",
    "BinaryIO",
    "Callable",
    "ConfigDict",
    "Cursor",
    "Dict",
    "Discriminator",
    "Field",
    "FileInput",
    "List",
    "Literal",
    "Mapping",
    "NotGiven",
    "Offset",
    "Optional",
    "Page",
    "RequestOptions",
    "Stream",
    "SyncPage",
    "Tag",
    "Union",
    "Unpack",
];

fn rename_type(t: &mut TypeRef, to: &BTreeMap<String, String>) {
    match t {
        TypeRef::Named { name } => {
            if let Some(new) = to.get(name) {
                *name = new.clone();
            }
        }
        TypeRef::Array { items: t }
        | TypeRef::Map { values: t }
        | TypeRef::Nullable { inner: t } => rename_type(t, to),
        TypeRef::Union { variants } => variants.iter_mut().for_each(|v| rename_type(v, to)),
        _ => {}
    }
}

/// Give every model that shares a name with something the generated code imports a `Model`
/// suffix, everywhere it is mentioned.
fn rename_types(api: &mut Api) {
    let client = pascal(&api.name);
    let mut taken: BTreeSet<String> = api.types.iter().map(|t| t.name.clone()).collect();
    let mut to = BTreeMap::new();
    for t in &api.types {
        if RESERVED_TYPES.contains(&t.name.as_str())
            || t.name == client
            || t.name == format!("Async{client}")
        {
            let mut new = format!("{}Model", t.name);
            while !taken.insert(new.clone()) {
                new.push('_');
            }
            to.insert(t.name.clone(), new);
        }
    }
    if to.is_empty() {
        return;
    }
    for t in &mut api.types {
        if let Some(new) = to.get(&t.name) {
            t.name = new.clone();
        }
        match &mut t.shape {
            Shape::Object { fields } => fields.iter_mut().for_each(|f| rename_type(&mut f.ty, &to)),
            Shape::Enum { .. } => {}
            Shape::Union {
                variants,
                discriminator,
            } => {
                variants.iter_mut().for_each(|v| rename_type(v, &to));
                for case in discriminator.iter_mut().flat_map(|d| &mut d.mapping) {
                    if let Some(new) = to.get(&case.type_name) {
                        case.type_name = new.clone();
                    }
                }
            }
            Shape::Alias { target } => rename_type(target, &to),
        }
    }
    for m in api.resources.iter_mut().flat_map(|r| &mut r.methods) {
        m.params
            .iter_mut()
            .for_each(|p| rename_type(&mut p.ty, &to));
        if let Some(b) = &mut m.body {
            rename_type(&mut b.ty, &to);
        }
        match &mut m.response {
            ResponseBody::Json { ty } => rename_type(ty, &to),
            ResponseBody::EventStream { event } => rename_type(event, &to),
            _ => {}
        }
        for ty in m.errors.iter_mut().filter_map(|e| e.ty.as_mut()) {
            rename_type(ty, &to);
        }
    }
    for w in &mut api.webhooks {
        rename_type(&mut w.payload, &to);
    }
}

/// `ident` for every name the emitters write as an identifier, so no emitter has to remember to.
/// Wire names are untouched; models and methods map them back with aliases and wire strings.
pub fn sanitize(api: &Api) -> Api {
    let mut api = api.clone();
    rename_types(&mut api);
    for r in &mut api.resources {
        r.name = ident(&r.name);
        for m in &mut r.methods {
            m.name = ident(&m.name);
            for p in &mut m.params {
                p.ident = ident(&p.ident);
            }
        }
    }
    for t in &mut api.types {
        if let Shape::Object { fields } = &mut t.shape {
            // `+1` and `-1` both snake_case to `1`; the later one gets `_2`.
            let mut taken = BTreeSet::new();
            for f in fields {
                let base = ident(&f.ident);
                f.ident = base.clone();
                let mut n = 1;
                while !taken.insert(f.ident.clone()) {
                    n += 1;
                    f.ident = format!("{base}_{n}");
                }
            }
        }
    }
    for w in &mut api.webhooks {
        w.ident = ident(&w.ident);
    }
    api
}

pub fn py_str(s: &str) -> String {
    format!(
        "\"{}\"",
        s.replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('\n', "\\n")
            .replace('\r', "\\r")
            .replace('\t', "\\t")
    )
}

/// A Python literal for a JSON value.
pub fn py_value(v: &Value) -> String {
    match v {
        Value::Null => "None".into(),
        Value::Bool(true) => "True".into(),
        Value::Bool(false) => "False".into(),
        Value::Number(n) => n.to_string(),
        Value::String(s) => py_str(s),
        Value::Array(a) => format!(
            "[{}]",
            a.iter().map(py_value).collect::<Vec<_>>().join(", ")
        ),
        Value::Object(o) => format!(
            "{{{}}}",
            o.iter()
                .map(|(k, v)| format!("{}: {}", py_str(k), py_value(v)))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

/// `("a", "b")`, with the trailing comma a one-element tuple needs.
pub fn py_tuple(items: &[String]) -> String {
    let parts: Vec<_> = items.iter().map(|i| py_str(i)).collect();
    match parts.as_slice() {
        [one] => format!("({one},)"),
        many => format!("({})", many.join(", ")),
    }
}

pub fn render(ty: &TypeRef, uses: &mut Uses, target: Target) -> String {
    match ty {
        TypeRef::String { format } => match format.as_deref() {
            Some("date-time") => {
                uses.datetime.insert("datetime");
                "datetime".into()
            }
            Some("date") => {
                uses.datetime.insert("date");
                "date".into()
            }
            _ => "str".into(),
        },
        TypeRef::Integer => "int".into(),
        TypeRef::Number => "float".into(),
        TypeRef::Boolean => "bool".into(),
        TypeRef::File => match target {
            Target::Sdk => {
                uses.named.insert("FileInput".into());
                "FileInput".into()
            }
            Target::Mcp => "str".into(),
        },
        TypeRef::Any => {
            uses.typing.insert("Any");
            "Any".into()
        }
        TypeRef::Array { items } => {
            uses.typing.insert("List");
            format!("List[{}]", render(items, uses, target))
        }
        TypeRef::Map { values } => {
            uses.typing.insert("Dict");
            format!("Dict[str, {}]", render(values, uses, target))
        }
        TypeRef::Enum { values } => enum_type(values, uses),
        TypeRef::Named { name } => {
            uses.named.insert(name.clone());
            name.clone()
        }
        TypeRef::Nullable { inner } => optional(render(inner, uses, target), uses),
        TypeRef::Union { variants } => match variants.as_slice() {
            [one] => render(one, uses, target),
            many => {
                uses.typing.insert("Union");
                let parts: Vec<_> = many.iter().map(|v| render(v, uses, target)).collect();
                format!("Union[{}]", parts.join(", "))
            }
        },
    }
}

/// An enum that keeps working when the server sends a value the spec doesn't list:
/// `Union[Literal["a", "b"], str]`. Editors still complete the known values.
pub fn enum_type(values: &[Value], uses: &mut Uses) -> String {
    let fallback = match values {
        [] => return "str".into(),
        v if v.iter().all(Value::is_i64) => "int",
        v if v.iter().all(Value::is_string) => "str",
        _ => {
            uses.typing.insert("Any");
            "Any"
        }
    };
    uses.typing.extend(["Literal", "Union"]);
    let parts: Vec<_> = values.iter().map(py_value).collect();
    format!("Union[Literal[{}], {fallback}]", parts.join(", "))
}

pub fn optional(inner: String, uses: &mut Uses) -> String {
    uses.typing.insert("Optional");
    format!("Optional[{inner}]")
}

/// Annotation for a model field: `Optional[T]` unless it is required or already nullable.
pub fn annotation(ty: &TypeRef, required: bool, uses: &mut Uses, target: Target) -> String {
    let rendered = render(ty, uses, target);
    if required || matches!(ty, TypeRef::Nullable { .. }) {
        rendered
    } else {
        optional(rendered, uses)
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Place {
    Path,
    Query,
    Header,
    Body,
}

/// One keyword argument of a generated method.
pub struct Arg<'a> {
    pub ident: String,
    /// Name on the wire (empty for a non-object `body`).
    pub wire: &'a str,
    pub place: Place,
    pub ty: &'a TypeRef,
    pub required: bool,
    pub description: Option<&'a str>,
    pub default: Option<&'a Value>,
}

/// Path, query and header params, then the body's fields flattened (a non-object body becomes
/// `body`, or `file` for raw bytes). Required arguments come first.
pub fn method_args<'a>(api: &'a Api, m: &'a Method) -> Vec<Arg<'a>> {
    let mut args: Vec<Arg> = Vec::new();
    for loc in [
        ParamLocation::Path,
        ParamLocation::Query,
        ParamLocation::Header,
    ] {
        for p in m.params.iter().filter(|p| p.location == loc) {
            args.push(Arg {
                ident: p.ident.clone(),
                wire: &p.name,
                place: match loc {
                    ParamLocation::Path => Place::Path,
                    ParamLocation::Query => Place::Query,
                    ParamLocation::Header => Place::Header,
                },
                ty: &p.ty,
                required: p.required,
                description: p.description.as_deref(),
                default: p.default.as_ref(),
            });
        }
    }
    if let Some(body) = &m.body {
        let fields = match &body.ty {
            TypeRef::Named { name } => match api.type_def(name).map(|t| &t.shape) {
                Some(Shape::Object { fields }) => Some(fields),
                _ => None,
            },
            _ => None,
        };
        match fields {
            Some(fields) => args.extend(fields.iter().map(|f| Arg {
                ident: f.ident.clone(),
                wire: &f.name,
                place: Place::Body,
                ty: &f.ty,
                required: f.required && body.required,
                description: f.description.as_deref(),
                default: f.default.as_ref(),
            })),
            None => args.push(Arg {
                ident: if body.encoding == BodyEncoding::Binary {
                    "file".into()
                } else {
                    "body".into()
                },
                wire: "",
                place: Place::Body,
                ty: &body.ty,
                required: body.required,
                description: None,
                default: None,
            }),
        }
    }
    // Parameters from different places (or a body field) may share a name; the first keeps it.
    // `self` and the per-request options are parameters of every method too.
    let mut taken: BTreeSet<String> = [
        "self",
        "options",
        "timeout",
        "max_retries",
        "extra_headers",
        "extra_query",
        "extra_body",
        "idempotency_key",
    ]
    .map(String::from)
    .into();
    for a in &mut args {
        let base = a.ident.clone();
        let mut n = 1;
        while !taken.insert(a.ident.clone()) {
            n += 1;
            a.ident = format!("{base}_{n}");
        }
    }
    args.sort_by_key(|a| !a.required);
    args
}

/// `ident: Annotation` or `ident: Annotation | NotGiven = NOT_GIVEN`. Omitting an optional
/// argument sends nothing; passing `None` sends `null`.
pub fn arg_decl(a: &Arg, uses: &mut Uses, target: Target) -> String {
    let ann = render(a.ty, uses, target);
    if a.required {
        format!("{}: {ann}", a.ident)
    } else {
        format!("{}: {ann} | NotGiven = NOT_GIVEN", a.ident)
    }
}

/// Summary, description and agent hint as paragraphs.
pub fn description(m: &Method) -> String {
    let mut parts: Vec<String> = Vec::new();
    match (&m.summary, &m.description) {
        (None, None) => parts.push(format!("{:?} {}.", m.http_method, m.path).to_uppercase()),
        (s, d) => parts.extend(s.iter().chain(d.iter()).map(|t| t.trim().to_string())),
    }
    if m.deprecated {
        parts.push("Deprecated: this operation may be removed.".into());
    }
    if let Some(h) = &m.agent.hint {
        parts.push(h.trim().to_string());
    }
    parts.join("\n\n")
}

pub fn is_binary(m: &Method) -> bool {
    matches!(m.response, ResponseBody::Binary { .. })
}

/// Text safe to place between `"""` delimiters.
pub fn escape_doc(text: &str) -> String {
    let mut text = text.replace('\\', "\\\\").replace("\"\"\"", "\\\"\\\"\\\"");
    // A quote against the opening or closing delimiter would merge with it.
    if text.starts_with('"') {
        text.insert(0, '\\');
    }
    if text.ends_with('"') {
        text.insert(text.len() - 1, '\\');
    }
    text
}

/// Docstring literal for `indent` spaces of indentation.
pub fn docstring(text: &str, indent: usize) -> String {
    let pad = " ".repeat(indent);
    let text = escape_doc(text);
    let lines: Vec<&str> = text.lines().collect();
    if lines.len() <= 1 {
        return format!("{pad}\"\"\"{}\"\"\"", lines.first().unwrap_or(&""));
    }
    let mut out = format!("{pad}\"\"\"{}\n", lines[0]);
    for l in &lines[1..] {
        if l.is_empty() {
            out.push('\n');
        } else {
            out.push_str(&format!("{pad}{l}\n"));
        }
    }
    out.push_str(&format!("{pad}\"\"\""));
    out
}

/// `from typing import ...` etc. as source lines. `write_all` runs `ruff` to sort them.
pub fn import_lines(uses: &Uses) -> Vec<String> {
    let mut out = Vec::new();
    if !uses.datetime.is_empty() {
        let names: Vec<_> = uses.datetime.iter().copied().collect();
        out.push(format!("from datetime import {}", names.join(", ")));
    }
    if !uses.typing.is_empty() {
        let names: Vec<_> = uses.typing.iter().copied().collect();
        out.push(format!("from typing import {}", names.join(", ")));
    }
    out
}
