//! Python-flavoured views of the IR shared by the SDK and MCP emitters.

use std::collections::BTreeSet;

use oryva_ir::{Api, Method, ParamLocation, ResponseBody, Shape, TypeRef};

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
        TypeRef::Enum { values } => literal(values, uses),
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

pub fn literal(values: &[String], uses: &mut Uses) -> String {
    if values.is_empty() {
        return "str".into();
    }
    uses.typing.insert("Literal");
    let parts: Vec<_> = values.iter().map(|v| py_str(v)).collect();
    format!("Literal[{}]", parts.join(", "))
}

pub fn optional(inner: String, uses: &mut Uses) -> String {
    uses.typing.insert("Optional");
    format!("Optional[{inner}]")
}

/// Annotation for something that may be omitted: `Optional[T]` unless it already is.
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
}

/// Path, query and header params, then the body's fields flattened (a non-object body becomes
/// `body`). Required arguments come first.
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
            })),
            None => args.push(Arg {
                ident: "body",
                wire: "",
                place: Place::Body,
                ty: &body.ty,
                required: body.required,
                description: None,
            }),
        }
    }
    args.sort_by_key(|a| !a.required);
    args
}

/// `ident: Annotation` or `ident: Annotation = None`.
pub fn arg_decl(a: &Arg, uses: &mut Uses, target: Target) -> String {
    let ann = annotation(a.ty, a.required, uses, target);
    if a.required {
        format!("{}: {ann}", a.ident)
    } else {
        format!("{}: {ann} = None", a.ident)
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

/// `from typing import ...` etc. as source lines.
pub fn import_lines(uses: &Uses) -> Vec<String> {
    let mut out = Vec::new();
    if !uses.datetime.is_empty() {
        let names: Vec<_> = uses.datetime.iter().copied().collect();
        out.push(format!("from datetime import {}", names.join(", ")));
    }
    if !uses.typing.is_empty() {
        // isort order: CONSTANTS, then Classes.
        let mut names: Vec<_> = uses.typing.iter().copied().collect();
        names.sort_by_key(|n| (!n.chars().all(|c| c.is_ascii_uppercase() || c == '_'), *n));
        out.push(format!("from typing import {}", names.join(", ")));
    }
    out
}
