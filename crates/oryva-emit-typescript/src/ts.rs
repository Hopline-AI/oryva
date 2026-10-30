//! TypeScript spellings: names, literals, types and doc comments.

use std::collections::BTreeSet;

use oryva_ir::{Field, Shape, TypeDef, TypeRef};
use serde_json::Value;

const RESERVED: &[&str] = &[
    "break",
    "case",
    "catch",
    "class",
    "const",
    "continue",
    "debugger",
    "default",
    "delete",
    "do",
    "else",
    "enum",
    "export",
    "extends",
    "false",
    "finally",
    "for",
    "function",
    "if",
    "import",
    "in",
    "instanceof",
    "new",
    "null",
    "return",
    "super",
    "switch",
    "this",
    "throw",
    "true",
    "try",
    "typeof",
    "var",
    "void",
    "while",
    "with",
    "yield",
    "let",
    "static",
    "await",
    "async",
    "implements",
    "interface",
    "package",
    "private",
    "protected",
    "public",
    "options",
    "params",
];

/// `download_pdf` -> `downloadPdf`.
pub fn camel(snake: &str) -> String {
    let mut out = String::new();
    for (i, part) in snake.split(['_', '-', '.']).enumerate() {
        let mut chars = part.chars();
        if let Some(first) = chars.next() {
            if i == 0 {
                out.push(first.to_ascii_lowercase());
            } else {
                out.push(first.to_ascii_uppercase());
            }
            out.push_str(chars.as_str());
        }
    }
    out
}

/// `download_pdf` -> `DownloadPdf`.
pub fn pascal(snake: &str) -> String {
    let c = camel(snake);
    let mut chars = c.chars();
    match chars.next() {
        Some(first) => first.to_ascii_uppercase().to_string() + chars.as_str(),
        None => c,
    }
}

/// A camelCase variable name that is safe to declare.
pub fn variable(snake: &str) -> String {
    let name = camel(snake);
    if RESERVED.contains(&name.as_str()) {
        format!("{name}_")
    } else {
        name
    }
}

/// A property key: bare when it is a valid identifier, quoted otherwise.
pub fn prop(name: &str) -> String {
    let ident = name.chars().enumerate().all(|(i, c)| {
        c == '_' || c == '$' || c.is_ascii_alphabetic() || (i > 0 && c.is_ascii_digit())
    });
    if ident && !name.is_empty() {
        name.to_string()
    } else {
        string(name)
    }
}

/// A single-quoted string literal.
pub fn string(s: &str) -> String {
    let mut out = String::from("'");
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\'' => out.push_str("\\'"),
            '\n' => out.push_str("\\n"),
            c => out.push(c),
        }
    }
    out.push('\'');
    out
}

/// A JSON value as a TypeScript literal.
pub fn literal(v: &Value) -> String {
    match v {
        Value::String(s) => string(s),
        other => other.to_string(),
    }
}

pub fn ty(t: &TypeRef) -> String {
    match t {
        TypeRef::String { .. } => "string".into(),
        TypeRef::Integer | TypeRef::Number => "number".into(),
        TypeRef::Boolean => "boolean".into(),
        TypeRef::File => "Uploadable".into(),
        TypeRef::Any => "unknown".into(),
        TypeRef::Array { items } => format!("Array<{}>", ty(items)),
        TypeRef::Map { values } => format!("{{ [key: string]: {} }}", ty(values)),
        TypeRef::Enum { values } => enum_union(values),
        TypeRef::Named { name } => name.clone(),
        TypeRef::Nullable { inner } => format!("{} | null", ty(inner)),
        TypeRef::Union { variants } => variants.iter().map(ty).collect::<Vec<_>>().join(" | "),
    }
}

/// `'a' | 'b' | (string & {})`: the last member keeps values added later from being type
/// errors. A single value is a constant (a discriminator), so it stays exact.
fn enum_union(values: &[Value]) -> String {
    let mut parts: Vec<String> = values.iter().map(literal).collect();
    if values.len() > 1 {
        let open = if values.iter().all(Value::is_string) {
            "(string & {})"
        } else {
            "(number & {})"
        };
        parts.push(open.into());
    }
    parts.join(" | ")
}

/// Names of the `TypeDef`s a type mentions.
pub fn named(t: &TypeRef, out: &mut BTreeSet<String>) {
    match t {
        TypeRef::Named { name } => {
            out.insert(name.clone());
        }
        TypeRef::Array { items } => named(items, out),
        TypeRef::Map { values } => named(values, out),
        TypeRef::Nullable { inner } => named(inner, out),
        TypeRef::Union { variants } => variants.iter().for_each(|v| named(v, out)),
        _ => {}
    }
}

/// A `/** ... */` block. Empty when there is nothing to say.
pub fn doc(paragraphs: &[String]) -> String {
    let lines: Vec<String> = paragraphs
        .iter()
        .filter(|p| !p.trim().is_empty())
        .flat_map(|p| {
            let mut lines: Vec<String> =
                p.trim().lines().map(|l| l.trim_end().to_string()).collect();
            lines.push(String::new());
            lines
        })
        .collect();
    let lines = &lines[..lines.len().saturating_sub(1)];
    match lines {
        [] => String::new(),
        [one] => format!("/** {} */\n", one.replace("*/", "*\\/")),
        many => {
            let body: Vec<String> = many
                .iter()
                .map(|l| {
                    format!(" * {}", l.replace("*/", "*\\/"))
                        .trim_end()
                        .to_string()
                })
                .collect();
            format!("/**\n{}\n */\n", body.join("\n"))
        }
    }
}

fn field_doc(description: &Option<String>, default: &Option<Value>) -> String {
    let mut paragraphs: Vec<String> = description.iter().cloned().collect();
    if let Some(d) = default {
        paragraphs.push(format!("@default {}", literal(d)));
    }
    doc(&paragraphs)
}

pub fn field(f: &Field) -> String {
    format!(
        "{}{}{}: {};\n",
        field_doc(&f.description, &f.default),
        prop(&f.name),
        if f.required { "" } else { "?" },
        ty(&f.ty)
    )
}

pub fn type_def(def: &TypeDef) -> String {
    let doc = doc(&def.description.iter().cloned().collect::<Vec<_>>());
    let name = &def.name;
    match &def.shape {
        Shape::Object { fields } => {
            let body: String = fields.iter().map(field).collect();
            format!("{doc}export interface {name} {{\n{body}}}\n")
        }
        Shape::Enum { values } => format!("{doc}export type {name} = {};\n", enum_union(values)),
        Shape::Union {
            variants,
            discriminator,
        } => {
            let note = match discriminator {
                Some(d) => format!(
                    "Discriminated by `{}`. A server newer than this SDK may send a value not listed here.\n",
                    d.property
                ),
                None => String::new(),
            };
            let doc = if def.description.is_some() || discriminator.is_some() {
                self::doc(&[def.description.clone().unwrap_or_default(), note])
            } else {
                doc
            };
            let union = variants.iter().map(ty).collect::<Vec<_>>().join(" | ");
            format!("{doc}export type {name} = {union};\n")
        }
        Shape::Alias { target } => format!("{doc}export type {name} = {};\n", ty(target)),
    }
}
