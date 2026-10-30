//! Builds the template contexts for the Python SDK.

use std::collections::BTreeSet;

use oryva_ir::{Api, Auth, BodyEncoding, Method, ResponseBody, Shape, TypeRef};
use serde::Serialize;

use crate::Options;
use crate::py::{
    Arg, Place, Target, Uses, annotation, arg_decl, description, docstring, import_lines,
    method_args, pascal, py_str, render,
};

#[derive(Serialize)]
pub struct Common {
    pub pkg: String,
    pub dist: String,
    pub version: String,
    pub class: String,
    pub error_base: String,
    pub title: String,
    pub summary: String,
    pub env_key: Option<String>,
    pub env_base_url: String,
    pub default_base_url: Option<String>,
    pub auth_header: Option<String>,
    pub auth_prefix: String,
    pub resources: Vec<ResourceRef>,
    pub example: String,
}

#[derive(Serialize)]
pub struct ResourceRef {
    pub module: String,
    pub class: String,
}

pub fn common(api: &Api, opts: &Options) -> Common {
    let (env_key, auth_header, auth_prefix) = match &api.auth {
        Some(Auth::ApiKey { header, env_var }) => {
            (Some(env_var.clone()), Some(header.clone()), String::new())
        }
        Some(Auth::Bearer { env_var }) => (
            Some(env_var.clone()),
            Some("Authorization".into()),
            "Bearer ".into(),
        ),
        None => (None, None, String::new()),
    };
    let class = pascal(&api.name);
    Common {
        pkg: opts.package.clone(),
        dist: opts.dist_name.clone(),
        version: opts.version.clone(),
        error_base: format!("{class}Error"),
        title: api.title.clone(),
        summary: api
            .description
            .clone()
            .unwrap_or_else(|| format!("Python client for the {}.", api.title)),
        env_key,
        env_base_url: format!("{}_BASE_URL", api.name.to_uppercase()),
        default_base_url: api.base_url.clone(),
        auth_header,
        auth_prefix,
        resources: api
            .resources
            .iter()
            .map(|r| ResourceRef {
                module: r.name.clone(),
                class: pascal(&r.name),
            })
            .collect(),
        example: readme_example(api, &class),
        class,
    }
}

// ---- types.py ----

#[derive(Serialize)]
pub struct TypesCtx {
    pub imports: Vec<String>,
    pub pydantic: String,
    pub types: Vec<TypeItem>,
}

#[derive(Serialize)]
pub struct TypeItem {
    pub name: String,
    pub doc: Option<String>,
    /// `Some` for aliases (`Name = expr`); `None` for models.
    pub expr: Option<String>,
    /// Arguments of the model's `ConfigDict(...)`.
    pub config: String,
    pub fields: Vec<FieldItem>,
}

#[derive(Serialize)]
pub struct FieldItem {
    pub ident: String,
    pub annotation: String,
    pub assign: Option<String>,
    pub doc: Option<String>,
}

pub fn types_ctx(api: &Api) -> TypesCtx {
    let mut uses = Uses::default();
    uses.typing.extend(["BinaryIO", "Union"]);
    let mut pydantic = vec!["BaseModel", "ConfigDict"];
    let mut items = Vec::new();
    for t in sorted_types(api) {
        let mut item = TypeItem {
            name: t.name.clone(),
            doc: t.description.as_deref().map(|d| d.trim().to_string()),
            expr: None,
            config: "extra=\"allow\", populate_by_name=True".into(),
            fields: vec![],
        };
        match &t.shape {
            Shape::Object { fields } => {
                if fields.iter().any(|f| has_file(&f.ty)) {
                    item.config.push_str(", arbitrary_types_allowed=True");
                }
                for f in fields {
                    let ann = annotation(&f.ty, f.required, &mut uses, Target::Sdk);
                    let alias = (f.ident != f.name).then(|| py_str(&f.name));
                    if alias.is_some() && !pydantic.contains(&"Field") {
                        pydantic.push("Field");
                    }
                    let assign = match (alias, f.required) {
                        (Some(a), true) => Some(format!("Field(alias={a})")),
                        (Some(a), false) => Some(format!("Field(None, alias={a})")),
                        (None, false) => Some("None".into()),
                        (None, true) => None,
                    };
                    item.fields.push(FieldItem {
                        ident: f.ident.clone(),
                        annotation: ann,
                        assign,
                        doc: f.description.as_deref().map(|d| d.trim().to_string()),
                    });
                }
            }
            Shape::Enum { values } => item.expr = Some(crate::py::literal(values, &mut uses)),
            Shape::Union { variants, .. } => {
                item.expr = Some(render(
                    &TypeRef::Union {
                        variants: variants.clone(),
                    },
                    &mut uses,
                    Target::Sdk,
                ))
            }
            Shape::Alias { target } => item.expr = Some(render(target, &mut uses, Target::Sdk)),
        }
        items.push(item);
    }
    pydantic.sort();
    TypesCtx {
        imports: import_lines(&uses),
        pydantic: pydantic.join(", "),
        types: items,
    }
}

fn has_file(ty: &TypeRef) -> bool {
    match ty {
        TypeRef::File => true,
        TypeRef::Array { items } => has_file(items),
        TypeRef::Map { values } => has_file(values),
        TypeRef::Nullable { inner } => has_file(inner),
        TypeRef::Union { variants } => variants.iter().any(has_file),
        _ => false,
    }
}

fn refs(ty: &TypeRef, out: &mut Vec<String>) {
    match ty {
        TypeRef::Named { name } => out.push(name.clone()),
        TypeRef::Array { items } => refs(items, out),
        TypeRef::Map { values } => refs(values, out),
        TypeRef::Nullable { inner } => refs(inner, out),
        TypeRef::Union { variants } => variants.iter().for_each(|v| refs(v, out)),
        _ => {}
    }
}

/// Types in dependency order, so aliases never name a type defined further down.
fn sorted_types(api: &Api) -> Vec<&oryva_ir::TypeDef> {
    fn visit<'a>(
        api: &'a Api,
        t: &'a oryva_ir::TypeDef,
        seen: &mut BTreeSet<String>,
        out: &mut Vec<&'a oryva_ir::TypeDef>,
    ) {
        if !seen.insert(t.name.clone()) {
            return;
        }
        let mut deps = Vec::new();
        match &t.shape {
            Shape::Object { fields } => fields.iter().for_each(|f| refs(&f.ty, &mut deps)),
            Shape::Union { variants, .. } => variants.iter().for_each(|v| refs(v, &mut deps)),
            Shape::Alias { target } => refs(target, &mut deps),
            Shape::Enum { .. } => {}
        }
        for d in deps {
            if let Some(dt) = api.type_def(&d) {
                visit(api, dt, seen, out);
            }
        }
        out.push(t);
    }
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for t in &api.types {
        visit(api, t, &mut seen, &mut out);
    }
    out
}

// ---- resources ----

#[derive(Serialize)]
pub struct ResourceCtx {
    pub class: String,
    pub client_class: String,
    pub doc: String,
    pub stdlib: Vec<String>,
    pub third_party: Vec<String>,
    pub types: Vec<String>,
    pub methods: Vec<MethodItem>,
}

#[derive(Serialize)]
pub struct MethodItem {
    pub name: String,
    pub args: Vec<String>,
    pub returns: String,
    pub doc: String,
    pub body: Vec<String>,
}

pub fn resource_ctx(api: &Api, r: &oryva_ir::Resource) -> ResourceCtx {
    let mut uses = Uses::default();
    uses.typing.insert("TYPE_CHECKING");
    let mut needs_quote = false;
    let mut needs_adapter = false;
    let methods = r
        .methods
        .iter()
        .map(|m| method_item(api, m, &mut uses, &mut needs_quote, &mut needs_adapter))
        .collect();
    let mut stdlib = import_lines(&uses);
    if needs_quote {
        stdlib.push("from urllib.parse import quote".into());
    }
    let third_party = if needs_adapter {
        vec!["from pydantic import TypeAdapter".into()]
    } else {
        vec![]
    };
    ResourceCtx {
        class: pascal(&r.name),
        client_class: pascal(&api.name),
        doc: r
            .description
            .clone()
            .unwrap_or_else(|| format!("{} endpoints.", humanize(&r.name))),
        stdlib,
        third_party,
        types: uses.named.into_iter().collect(),
        methods,
    }
}

pub fn humanize(snake: &str) -> String {
    let s = snake.replace('_', " ");
    let mut c = s.chars();
    c.next()
        .map(|f| f.to_uppercase().chain(c).collect())
        .unwrap_or_default()
}

fn is_file(ty: &TypeRef) -> bool {
    match ty {
        TypeRef::File => true,
        TypeRef::Nullable { inner } => is_file(inner),
        _ => false,
    }
}

fn dict(name: &str, entries: &[String]) -> Vec<String> {
    match entries {
        [one] => vec![format!("{name} = {{{one}}}")],
        many => {
            let mut v = vec![format!("{name} = {{")];
            v.extend(many.iter().map(|e| format!("    {e},")));
            v.push("}".into());
            v
        }
    }
}

fn entries(args: &[&Arg]) -> Vec<String> {
    args.iter()
        .map(|a| format!("{}: {}", py_str(a.wire), a.ident))
        .collect()
}

fn error_class(status: u16) -> Option<&'static str> {
    Some(match status {
        400 => "BadRequestError",
        401 => "AuthenticationError",
        403 => "PermissionDeniedError",
        404 => "NotFoundError",
        422 => "UnprocessableEntityError",
        429 => "RateLimitError",
        500..=599 => "InternalServerError",
        _ => return None,
    })
}

fn one_line(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn method_item(
    api: &Api,
    m: &Method,
    uses: &mut Uses,
    needs_quote: &mut bool,
    needs_adapter: &mut bool,
) -> MethodItem {
    let args = method_args(api, m);
    let decls: Vec<String> = args
        .iter()
        .map(|a| arg_decl(a, uses, Target::Sdk))
        .collect();

    // Docstring: description, then Args and Raises.
    let mut doc = description(m);
    let documented: Vec<String> = args
        .iter()
        .filter_map(|a| {
            a.description
                .map(|d| format!("    {}: {}", a.ident, one_line(d)))
        })
        .collect();
    if !documented.is_empty() {
        doc.push_str(&format!("\n\nArgs:\n{}", documented.join("\n")));
    }
    let mut raises: Vec<String> = Vec::new();
    for e in &m.errors {
        if let Some(class) = error_class(e.status) {
            let line = format!(
                "    {class}: {}",
                e.description
                    .as_deref()
                    .map(one_line)
                    .unwrap_or_else(|| format!("The API returned {}.", e.status))
            );
            if !raises
                .iter()
                .any(|r| r.starts_with(&format!("    {class}:")))
            {
                raises.push(line);
            }
        }
    }
    if !raises.is_empty() {
        doc.push_str(&format!("\n\nRaises:\n{}", raises.join("\n")));
    }

    let mut body: Vec<String> = Vec::new();
    let mut call: Vec<String> = vec![py_str(&format!("{:?}", m.http_method).to_uppercase())];

    // Path: an f-string when it has parameters.
    let mut path = m.path.clone();
    for a in args.iter().filter(|a| a.place == Place::Path) {
        let value = match a.ty {
            TypeRef::String { .. } => a.ident.to_string(),
            _ => format!("str({})", a.ident),
        };
        path = path.replace(
            &format!("{{{}}}", a.wire),
            &format!("{{quote({value}, safe='')}}"),
        );
        *needs_quote = true;
    }
    if path == m.path {
        call.push(py_str(&path));
    } else {
        body.push(format!("path = f{}", py_str(&path)));
        call.push("path".into());
    }

    let query: Vec<&Arg> = args.iter().filter(|a| a.place == Place::Query).collect();
    if !query.is_empty() {
        body.extend(dict("params", &entries(&query)));
        call.push("params=params".into());
    }
    let header: Vec<&Arg> = args.iter().filter(|a| a.place == Place::Header).collect();
    if !header.is_empty() {
        body.extend(dict("headers", &entries(&header)));
        call.push("headers=headers".into());
    }
    if let Some(b) = &m.body {
        let fields: Vec<&Arg> = args.iter().filter(|a| a.place == Place::Body).collect();
        let flat = fields.first().is_none_or(|a| !a.wire.is_empty());
        match b.encoding {
            BodyEncoding::Json | BodyEncoding::Form => {
                let (var, kw) = if b.encoding == BodyEncoding::Json {
                    ("body", "json")
                } else {
                    ("form", "data")
                };
                if flat {
                    body.extend(dict(var, &entries(&fields)));
                    call.push(format!("{kw}={var}"));
                } else {
                    call.push(format!("{kw}=body"));
                }
            }
            BodyEncoding::Multipart => {
                let (files, data): (Vec<&Arg>, Vec<&Arg>) =
                    fields.into_iter().partition(|a| is_file(a.ty));
                if !files.is_empty() {
                    body.extend(dict("files", &entries(&files)));
                    call.push("files=files".into());
                }
                if !data.is_empty() {
                    body.extend(dict("data", &entries(&data)));
                    call.push("data=data".into());
                }
            }
        }
    }

    let request = if call.len() <= 3 {
        format!("self._client._request({})", call.join(", "))
    } else {
        let mut s = String::from("self._client._request(\n");
        for c in &call {
            s.push_str(&format!("    {c},\n"));
        }
        s.push(')');
        s
    };
    let (returns, tail): (String, Vec<String>) = match &m.response {
        ResponseBody::Empty => {
            body.extend(request.lines().map(String::from));
            ("None".into(), vec![])
        }
        other => {
            let mut lines = request.lines().map(String::from);
            body.push(format!("response = {}", lines.next().unwrap_or_default()));
            body.extend(lines);
            match other {
                ResponseBody::Binary { .. } => {
                    ("bytes".into(), vec!["return response.content".into()])
                }
                ResponseBody::Text => ("str".into(), vec!["return response.text".into()]),
                ResponseBody::Json { ty } => {
                    let ann = render(ty, uses, Target::Sdk);
                    let is_model = matches!(ty, TypeRef::Named { name }
                        if matches!(api.type_def(name).map(|t| &t.shape), Some(Shape::Object { .. })));
                    let ret = if is_model {
                        format!("return {ann}.model_validate(response.json())")
                    } else if matches!(ty, TypeRef::Any) {
                        "return response.json()".to_string()
                    } else {
                        *needs_adapter = true;
                        format!("return TypeAdapter({ann}).validate_python(response.json())")
                    };
                    (ann, vec![ret])
                }
                ResponseBody::Empty => unreachable!(),
            }
        }
    };
    body.extend(tail);

    MethodItem {
        name: m.name.clone(),
        args: decls,
        returns,
        doc: docstring(&doc, 8),
        body,
    }
}

// ---- README ----

fn example_value(api: &Api, ty: &TypeRef) -> String {
    match ty {
        TypeRef::String { .. } => "\"...\"".into(),
        TypeRef::Integer => "1".into(),
        TypeRef::Number => "1.0".into(),
        TypeRef::Boolean => "True".into(),
        TypeRef::File => "open(\"sample.wav\", \"rb\")".into(),
        TypeRef::Any => "None".into(),
        TypeRef::Array { .. } => "[]".into(),
        TypeRef::Map { .. } => "{}".into(),
        TypeRef::Enum { values } => values
            .first()
            .map(|v| py_str(v))
            .unwrap_or("\"...\"".into()),
        TypeRef::Nullable { inner } => example_value(api, inner),
        TypeRef::Union { variants } => variants
            .first()
            .map(|v| example_value(api, v))
            .unwrap_or("None".into()),
        TypeRef::Named { name } => match api.type_def(name).map(|t| &t.shape) {
            Some(Shape::Enum { values }) => values
                .first()
                .map(|v| py_str(v))
                .unwrap_or("\"...\"".into()),
            Some(Shape::Alias { target }) => example_value(api, target),
            Some(Shape::Union { variants, .. }) => variants
                .first()
                .map(|v| example_value(api, v))
                .unwrap_or("None".into()),
            _ => "{}".into(),
        },
    }
}

fn readme_example(api: &Api, class: &str) -> String {
    let Some((r, m)) = api
        .resources
        .iter()
        .find_map(|r| r.methods.first().map(|m| (r, m)))
    else {
        return format!("client = {class}()");
    };
    let args: Vec<String> = method_args(api, m)
        .iter()
        .filter(|a| a.required)
        .map(|a| format!("    {}={},", a.ident, example_value(api, a.ty)))
        .collect();
    let call = if args.is_empty() {
        format!("client.{}.{}()", r.name, m.name)
    } else {
        format!("client.{}.{}(\n{}\n)", r.name, m.name, args.join("\n"))
    };
    match m.response {
        ResponseBody::Empty => format!("client = {class}()\n{call}"),
        _ => format!("client = {class}()\nresult = {call}\nprint(result)"),
    }
}
