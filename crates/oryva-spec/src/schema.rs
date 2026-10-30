//! Schema → `TypeRef`/`TypeDef`, including `$ref` resolution and hoisting of inline objects.

use crate::invalid;
use heck::{ToSnakeCase, ToUpperCamelCase};
use oryva_ir::{Field, Shape, TypeDef, TypeRef};
use serde_json::Value;
use std::collections::BTreeMap;

use crate::SpecError;

pub(crate) struct Ctx<'a> {
    pub root: &'a Value,
    pub types: BTreeMap<String, TypeDef>,
}

/// What a schema is before it is given a name.
enum Node {
    Object(Vec<Field>),
    Enum(Vec<String>),
    Union(Vec<TypeRef>, Option<String>),
    Plain(TypeRef),
}

/// Escape a key for use in a JSON pointer.
pub(crate) fn esc(s: &str) -> String {
    s.replace('~', "~0").replace('/', "~1")
}

pub(crate) fn pascal(s: &str) -> String {
    s.to_upper_camel_case()
}

pub(crate) fn string_at(v: &Value, key: &str) -> Option<String> {
    v.get(key).and_then(Value::as_str).map(String::from)
}

fn is_null_schema(v: &Value) -> bool {
    match v.get("type") {
        Some(Value::String(t)) => t == "null",
        Some(Value::Array(a)) => a.len() == 1 && a[0] == "null",
        _ => false,
    }
}

fn nullable(t: TypeRef) -> TypeRef {
    match t {
        TypeRef::Nullable { .. } | TypeRef::Any => t,
        t => TypeRef::Nullable { inner: Box::new(t) },
    }
}

impl<'a> Ctx<'a> {
    pub fn new(root: &'a Value) -> Self {
        Ctx {
            root,
            types: BTreeMap::new(),
        }
    }

    /// Check a `$ref` is local, points into `#/components/<section>` and exists.
    fn lookup(&self, r: &str, section: &str, at: &str) -> Result<&'a Value, SpecError> {
        if !r.starts_with('#') {
            return invalid(
                at,
                format!("external $ref {r:?} is not supported; bundle the spec into one file"),
            );
        }
        if !r.starts_with(&format!("#/components/{section}/")) {
            return invalid(
                at,
                format!("$ref {r:?} must point into #/components/{section}/"),
            );
        }
        match self.root.pointer(&r[1..]) {
            Some(v) => Ok(v),
            None => invalid(at, format!("$ref {r:?} does not resolve")),
        }
    }

    /// Follow `$ref`s until a concrete value. Returns it with the pointer it lives at.
    pub fn resolve(
        &self,
        v: &'a Value,
        at: &str,
        section: &str,
    ) -> Result<(&'a Value, String), SpecError> {
        let (mut v, mut at) = (v, at.to_string());
        for _ in 0..16 {
            let Some(r) = v.get("$ref") else {
                return Ok((v, at));
            };
            let Some(r) = r.as_str() else {
                return invalid(&at, "`$ref` must be a string");
            };
            v = self.lookup(r, section, &at)?;
            at = r.to_string();
        }
        invalid(&at, "`$ref` chain is too deep; it probably loops")
    }

    fn schema_ref(&self, r: &str, at: &str) -> Result<TypeRef, SpecError> {
        self.lookup(r, "schemas", at)?;
        let key = r["#/components/schemas/".len()..]
            .replace("~1", "/")
            .replace("~0", "~");
        Ok(TypeRef::Named { name: pascal(&key) })
    }

    pub fn components(&mut self) -> Result<(), SpecError> {
        let Some(schemas) = self
            .root
            .pointer("/components/schemas")
            .and_then(Value::as_object)
        else {
            return Ok(());
        };
        for (key, s) in schemas {
            let name = pascal(key);
            let at = format!("#/components/schemas/{}", esc(key));
            let (node, null) = self.node(s, &name, &at, true)?;
            let shape = match node {
                // A nullable object/enum/union component loses its nullability here; the IR
                // has no way to say it on a `Named` reference.
                Node::Object(fields) => Shape::Object { fields },
                Node::Enum(values) => Shape::Enum { values },
                Node::Union(variants, discriminator) => Shape::Union {
                    variants,
                    discriminator,
                },
                Node::Plain(t) => Shape::Alias {
                    target: if null { nullable(t) } else { t },
                },
            };
            self.add(&name, s, shape, &at)?;
        }
        Ok(())
    }

    fn add(&mut self, name: &str, s: &Value, shape: Shape, at: &str) -> Result<(), SpecError> {
        if self.types.contains_key(name) {
            return invalid(
                at,
                format!(
                    "the generated type name `{name}` is already taken by a component schema or another inline object; rename one of them"
                ),
            );
        }
        self.types.insert(
            name.to_string(),
            TypeDef {
                name: name.to_string(),
                description: string_at(s, "description"),
                shape,
            },
        );
        Ok(())
    }

    /// Convert a schema used inline. Objects are hoisted into a type called `hint`.
    pub fn ty(&mut self, s: &'a Value, hint: &str, at: &str) -> Result<TypeRef, SpecError> {
        let (node, null) = self.node(s, hint, at, false)?;
        self.realize(node, null, hint, s, at)
    }

    fn realize(
        &mut self,
        node: Node,
        null: bool,
        hint: &str,
        s: &Value,
        at: &str,
    ) -> Result<TypeRef, SpecError> {
        let t = match node {
            Node::Object(fields) => {
                self.add(hint, s, Shape::Object { fields }, at)?;
                TypeRef::Named { name: hint.into() }
            }
            Node::Enum(values) => TypeRef::Enum { values },
            Node::Union(variants, _) => TypeRef::Union { variants },
            Node::Plain(t) => t,
        };
        Ok(if null { nullable(t) } else { t })
    }

    /// `top` is true for a component schema, whose own name is `hint`.
    fn node(
        &mut self,
        s: &'a Value,
        hint: &str,
        at: &str,
        top: bool,
    ) -> Result<(Node, bool), SpecError> {
        if !s.is_object() {
            return Ok((Node::Plain(TypeRef::Any), false));
        }
        if let Some(r) = s.get("$ref") {
            let Some(r) = r.as_str() else {
                return invalid(at, "`$ref` must be a string");
            };
            return Ok((Node::Plain(self.schema_ref(r, at)?), false));
        }
        let mut null = s.get("nullable").and_then(Value::as_bool).unwrap_or(false);

        if let Some(parts) = s.get("allOf").and_then(Value::as_array) {
            if let [only] = parts.as_slice()
                && s.get("properties").is_none()
                && let Some(r) = only.get("$ref").and_then(Value::as_str)
            {
                let t = self.schema_ref(r, &format!("{at}/allOf/0"))?;
                return Ok((Node::Plain(t), null));
            }
            let (mut props, mut required) = (Vec::new(), Vec::new());
            self.collect(s, at, 0, &mut props, &mut required)?;
            let fields = self.fields(props, &required, hint, at)?;
            return Ok((Node::Object(fields), null));
        }

        for key in ["oneOf", "anyOf"] {
            let Some(all) = s.get(key).and_then(Value::as_array) else {
                continue;
            };
            let live: Vec<(usize, &'a Value)> = all
                .iter()
                .enumerate()
                .filter(|(_, v)| !is_null_schema(v))
                .collect();
            null |= live.len() < all.len();
            let single = live.len() == 1;
            let mut variants = Vec::new();
            for (n, (i, v)) in live.into_iter().enumerate() {
                let h = if single && !top {
                    hint.to_string()
                } else {
                    format!("{hint}Variant{}", n + 1)
                };
                variants.push(self.ty(v, &h, &format!("{at}/{key}/{i}"))?);
            }
            let node = match variants.len() {
                0 => Node::Plain(TypeRef::Any),
                1 => Node::Plain(variants.remove(0)),
                _ => Node::Union(variants, string_at(&s["discriminator"], "propertyName")),
            };
            return Ok((node, null));
        }

        let literals: Option<Vec<&Value>> = match s.get("enum").and_then(Value::as_array) {
            Some(a) => Some(a.iter().collect()),
            None => s.get("const").map(|c| vec![c]),
        };
        if let Some(lits) = literals {
            null |= lits.iter().any(|v| v.is_null());
            let strings: Vec<String> = lits
                .iter()
                .filter(|v| !v.is_null())
                .filter_map(|v| v.as_str().map(String::from))
                .collect();
            let non_null = lits.iter().filter(|v| !v.is_null()).count();
            // Enums of numbers fall through and become plain integers or numbers.
            if !strings.is_empty() && strings.len() == non_null {
                return Ok((Node::Enum(strings), null));
            }
        }

        let mut kinds: Vec<&str> = match s.get("type") {
            Some(Value::String(t)) => vec![t],
            Some(Value::Array(a)) => a.iter().filter_map(Value::as_str).collect(),
            _ => vec![],
        };
        if kinds.contains(&"null") {
            null = true;
            kinds.retain(|k| *k != "null");
        }
        if kinds.is_empty() {
            if s.get("properties").is_some() || s.get("additionalProperties").is_some() {
                kinds.push("object");
            } else if s.get("items").is_some() {
                kinds.push("array");
            }
        }
        match kinds.as_slice() {
            [] => Ok((Node::Plain(TypeRef::Any), null)),
            [one] => Ok((self.kind(one, s, hint, at)?, null)),
            many => {
                let mut variants = Vec::new();
                for k in many {
                    let h = format!("{hint}{}", pascal(k));
                    let node = self.kind(k, s, &h, at)?;
                    variants.push(self.realize(node, false, &h, s, at)?);
                }
                Ok((Node::Union(variants, None), null))
            }
        }
    }

    fn kind(&mut self, kind: &str, s: &'a Value, hint: &str, at: &str) -> Result<Node, SpecError> {
        let plain = |t| Ok(Node::Plain(t));
        match kind {
            "string" => {
                let format = string_at(s, "format");
                if format.as_deref() == Some("binary")
                    || string_at(s, "contentMediaType").as_deref()
                        == Some("application/octet-stream")
                {
                    return plain(TypeRef::File);
                }
                plain(TypeRef::String { format })
            }
            "integer" => plain(TypeRef::Integer),
            "number" => plain(TypeRef::Number),
            "boolean" => plain(TypeRef::Boolean),
            "array" => {
                let items = match s.get("items") {
                    Some(i) => self.ty(i, &format!("{hint}Item"), &format!("{at}/items"))?,
                    None => TypeRef::Any,
                };
                plain(TypeRef::Array {
                    items: Box::new(items),
                })
            }
            "object" => {
                let mut props = Vec::new();
                self.collect(s, at, 0, &mut props, &mut Vec::new())?;
                if !props.is_empty() {
                    let required = required_of(s);
                    return Ok(Node::Object(self.fields(props, &required, hint, at)?));
                }
                // An object with both `properties` and `additionalProperties` keeps only the
                // properties.
                let values = match s.get("additionalProperties") {
                    Some(v @ Value::Object(_)) => self.ty(
                        v,
                        &format!("{hint}Value"),
                        &format!("{at}/additionalProperties"),
                    )?,
                    _ => TypeRef::Any,
                };
                plain(TypeRef::Map {
                    values: Box::new(values),
                })
            }
            _ => plain(TypeRef::Any),
        }
    }

    /// Gather properties and required names from a schema and everything in its `allOf`.
    /// Later definitions of a property replace earlier ones.
    fn collect(
        &self,
        s: &'a Value,
        at: &str,
        depth: usize,
        props: &mut Vec<(String, &'a Value)>,
        required: &mut Vec<String>,
    ) -> Result<(), SpecError> {
        if depth > 16 {
            return invalid(at, "`allOf` nests too deep; it probably loops");
        }
        let (s, at) = self.resolve(s, at, "schemas")?;
        for (i, part) in s
            .get("allOf")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .enumerate()
        {
            self.collect(part, &format!("{at}/allOf/{i}"), depth + 1, props, required)?;
        }
        for (k, v) in s
            .get("properties")
            .and_then(Value::as_object)
            .into_iter()
            .flatten()
        {
            match props.iter_mut().find(|(n, _)| n == k) {
                Some(slot) => slot.1 = v,
                None => props.push((k.clone(), v)),
            }
        }
        required.extend(required_of(s));
        Ok(())
    }

    fn fields(
        &mut self,
        props: Vec<(String, &'a Value)>,
        required: &[String],
        hint: &str,
        at: &str,
    ) -> Result<Vec<Field>, SpecError> {
        let mut out = Vec::new();
        for (name, schema) in props {
            let ty = self.ty(
                schema,
                &format!("{hint}{}", pascal(&name)),
                &format!("{at}/properties/{}", esc(&name)),
            )?;
            out.push(Field {
                ident: name.to_snake_case(),
                required: required.contains(&name),
                description: string_at(schema, "description"),
                ty,
                name,
            });
        }
        Ok(out)
    }
}

fn required_of(s: &Value) -> Vec<String> {
    s.get("required")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|v| v.as_str().map(String::from))
        .collect()
}
