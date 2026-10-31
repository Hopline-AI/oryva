//! Python-flavoured views of the IR shared by the SDK and MCP emitters.

use std::collections::BTreeSet;

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
    pub ident: &'a str,
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
                ident: &p.ident,
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
                ident: &f.ident,
                wire: &f.name,
                place: Place::Body,
                ty: &f.ty,
                required: f.required && body.required,
                description: f.description.as_deref(),
                default: f.default.as_ref(),
            })),
            None => args.push(Arg {
                ident: if body.encoding == BodyEncoding::Binary {
                    "file"
                } else {
                    "body"
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

/// Docstring literal for `indent` spaces of indentation.
pub fn docstring(text: &str, indent: usize) -> String {
    let pad = " ".repeat(indent);
    let text = text.replace('\\', "\\\\").replace("\"\"\"", "\\\"\\\"\\\"");
    let lines: Vec<&str> = text.lines().collect();
    if lines.len() == 1 {
        return format!("{pad}\"\"\"{}\"\"\"", lines[0]);
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
