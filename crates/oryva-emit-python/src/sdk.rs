//! Builds the template contexts for the Python SDK.

use std::collections::BTreeSet;

use oryva_ir::{
    Api, Auth, BodyEncoding, Method, Pagination, ResponseBody, Shape, TypeDef, TypeRef,
    WebhookSigning,
};
use serde::Serialize;

use crate::Options;
use crate::py::{
    Arg, Place, Target, Uses, annotation, arg_decl, description, docstring, import_lines,
    method_args, pascal, py_str, py_tuple, py_value, render,
};

#[derive(Serialize)]
pub struct Common {
    pub pkg: String,
    pub dist: String,
    pub version: String,
    pub class: String,
    pub async_class: String,
    pub error_base: String,
    pub title: String,
    pub summary: String,
    pub env_base_url: String,
    pub env_log: String,
    pub servers: Vec<ServerCtx>,
    pub default_env: Option<String>,
    pub auth: Vec<AuthCtx>,
    /// Every credential argument of the client constructors, in declaration order.
    pub ctor_args: Vec<CtorArg>,
    pub auth_help: String,
    pub idempotency_header: Option<String>,
    pub secret_headers: Vec<String>,
    pub secret_params: Vec<String>,
    pub webhook_secret_env: Option<String>,
    pub has_webhooks: bool,
    pub resources: Vec<ResourceRef>,
    pub example: String,
}

#[derive(Serialize)]
pub struct ServerCtx {
    pub name: String,
    pub url: String,
}

#[derive(Serialize, Clone)]
pub struct CtorArg {
    pub ident: String,
    pub env: String,
    pub doc: String,
}

#[derive(Serialize)]
pub struct AuthCtx {
    /// `api_key`, `bearer`, `basic` or `oauth2`.
    pub kind: &'static str,
    pub location: String,
    pub name: String,
    /// Python list literal.
    pub scopes: String,
    pub token_url: String,
    pub args: Vec<CtorArg>,
}

#[derive(Serialize)]
pub struct ResourceRef {
    pub module: String,
    pub class: String,
    pub async_class: String,
    pub raw_class: Option<String>,
    pub async_raw_class: Option<String>,
}

fn unique_arg(args: &mut Vec<CtorArg>, prefix: &str, env: &str, doc: &str) -> CtorArg {
    let base = env.strip_prefix(prefix).unwrap_or(env).to_lowercase();
    let mut ident = base.clone();
    let mut n = 1;
    while args.iter().any(|a| a.ident == ident) {
        n += 1;
        ident = format!("{base}_{n}");
    }
    let arg = CtorArg {
        ident,
        env: env.to_string(),
        doc: doc.to_string(),
    };
    args.push(arg.clone());
    arg
}

fn auth_ctx(api: &Api) -> (Vec<AuthCtx>, Vec<CtorArg>, String) {
    let prefix = format!("{}_", api.name.to_uppercase());
    let mut all = Vec::new();
    let mut schemes = Vec::new();
    let mut help = Vec::new();
    for a in &api.auth {
        let mut ctx = AuthCtx {
            kind: "",
            location: String::new(),
            name: String::new(),
            scopes: "[]".into(),
            token_url: String::new(),
            args: vec![],
        };
        match a {
            Auth::ApiKey {
                location,
                name,
                env_var,
            } => {
                ctx.kind = "api_key";
                ctx.location = format!("{location:?}").to_lowercase();
                ctx.name = name.clone();
                let arg = unique_arg(
                    &mut all,
                    &prefix,
                    env_var,
                    &format!("API key, sent in the `{name}` {}.", ctx.location),
                );
                help.push(format!("{}= (or set {env_var})", arg.ident));
                ctx.args = vec![arg];
            }
            Auth::Bearer { env_var } => {
                ctx.kind = "bearer";
                let arg = unique_arg(&mut all, &prefix, env_var, "Bearer token.");
                help.push(format!("{}= (or set {env_var})", arg.ident));
                ctx.args = vec![arg];
            }
            Auth::Basic {
                username_env,
                password_env,
            } => {
                ctx.kind = "basic";
                let user = unique_arg(&mut all, &prefix, username_env, "Username for basic auth.");
                let pass = unique_arg(&mut all, &prefix, password_env, "Password for basic auth.");
                help.push(format!(
                    "{}= and {}= (or set {username_env} and {password_env})",
                    user.ident, pass.ident
                ));
                ctx.args = vec![user, pass];
            }
            Auth::OAuth2ClientCredentials {
                token_url,
                scopes,
                client_id_env,
                client_secret_env,
            } => {
                ctx.kind = "oauth2";
                ctx.token_url = token_url.clone();
                ctx.scopes = format!(
                    "[{}]",
                    scopes
                        .iter()
                        .map(|s| py_str(s))
                        .collect::<Vec<_>>()
                        .join(", ")
                );
                let id = unique_arg(&mut all, &prefix, client_id_env, "OAuth2 client ID.");
                let secret = unique_arg(
                    &mut all,
                    &prefix,
                    client_secret_env,
                    "OAuth2 client secret.",
                );
                let url_env = format!("{prefix}OAUTH_TOKEN_URL");
                let mut url = unique_arg(
                    &mut all,
                    &prefix,
                    &url_env,
                    &format!("OAuth2 token endpoint. Defaults to {token_url}."),
                );
                // `token_url=`, not `oauth_token_url=`, like the spec calls it.
                if url.ident == "oauth_token_url" && !all.iter().any(|a| a.ident == "token_url") {
                    url.ident = "token_url".into();
                    if let Some(last) = all.last_mut() {
                        last.ident = "token_url".into();
                    }
                }
                help.push(format!(
                    "{}= and {}= (or set {client_id_env} and {client_secret_env})",
                    id.ident, secret.ident
                ));
                ctx.args = vec![id, secret, url];
            }
        }
        schemes.push(ctx);
    }
    let help = if help.is_empty() {
        String::new()
    } else {
        format!("No credentials found. Pass {}.", help.join(", or "))
    };
    (schemes, all, help)
}

pub fn common(api: &Api, opts: &Options) -> Common {
    let class = pascal(&api.name);
    let upper = api.name.to_uppercase();
    let (auth, ctor_args, auth_help) = auth_ctx(api);
    let mut secret_headers = vec!["authorization".to_string(), "cookie".into()];
    let mut secret_params = Vec::new();
    for a in &api.auth {
        if let Auth::ApiKey { location, name, .. } = a {
            match format!("{location:?}").as_str() {
                "Header" => secret_headers.push(name.to_lowercase()),
                "Query" => secret_params.push(name.clone()),
                _ => {}
            }
        }
    }
    let mut resources: Vec<ResourceRef> = api
        .resources
        .iter()
        .map(|r| {
            let c = pascal(&r.name);
            ResourceRef {
                module: r.name.clone(),
                async_class: format!("Async{c}"),
                raw_class: Some(format!("{c}WithRawResponse")),
                async_raw_class: Some(format!("Async{c}WithRawResponse")),
                class: c,
            }
        })
        .collect();
    let has_webhooks = !api.webhooks.is_empty();
    if has_webhooks {
        resources.push(ResourceRef {
            module: "webhooks".into(),
            class: "Webhooks".into(),
            async_class: "Webhooks".into(),
            raw_class: None,
            async_raw_class: None,
        });
    }
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
        env_base_url: format!("{upper}_BASE_URL"),
        env_log: format!("{upper}_LOG"),
        servers: api
            .servers
            .iter()
            .map(|s| ServerCtx {
                name: s.name.clone(),
                url: s.url.trim_end_matches('/').to_string(),
            })
            .collect(),
        default_env: api.servers.first().map(|s| s.name.clone()),
        auth,
        ctor_args,
        auth_help,
        idempotency_header: api.idempotency_header.clone(),
        secret_headers,
        secret_params,
        webhook_secret_env: api.webhook_signing.as_ref().map(|s| match s {
            WebhookSigning::StandardWebhooks { secret_env } => secret_env.clone(),
        }),
        has_webhooks,
        resources,
        example: readme_example(api, &class),
        async_class: format!("Async{class}"),
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
    /// Complete source, for types that need helper code around them.
    pub raw: Option<String>,
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

fn snake(pascal: &str) -> String {
    let mut out = String::new();
    for (i, c) in pascal.chars().enumerate() {
        if c.is_uppercase() && i > 0 {
            out.push('_');
        }
        out.extend(c.to_lowercase());
    }
    out
}

/// A union that picks its variant by a property's value and keeps unknown values as
/// `Unknown<Name>` instead of raising.
fn tagged_union(
    name: &str,
    property: &str,
    cases: &[(String, String)],
    doc: &str,
    uses: &mut Uses,
) -> String {
    uses.typing
        .extend(["Annotated", "Any", "Dict", "Optional", "Union"]);
    let unknown = format!("Unknown{name}");
    let ident: String = property
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '_' })
        .collect();
    let tags = format!("_{}_TAGS", snake(name).to_uppercase());
    let func = format!("_{}_tag", snake(name));
    let entries: Vec<_> = cases
        .iter()
        .map(|(value, ty)| format!("{}: {}", py_str(value), py_str(ty)))
        .collect();
    let mut variants: Vec<String> = Vec::new();
    for (_, ty) in cases {
        let v = format!("Annotated[{ty}, Tag({})]", py_str(ty));
        if !variants.contains(&v) {
            variants.push(v);
        }
    }
    let unknown_tag = py_str(&unknown);
    variants.push(format!("Annotated[{unknown}, Tag({unknown_tag})]"));
    format!(
        "class {unknown}(BaseModel):\n    \"\"\"Any `{doc}` variant this SDK version doesn't know yet, with its fields kept as extras.\"\"\"\n\n    model_config = ConfigDict(extra=\"allow\", populate_by_name=True)\n\n    {ident}: Optional[str] = None\n\n\n{tags}: Dict[str, str] = {{{}}}\n\n\ndef {func}(value: Any) -> str:\n    kind = value.get({}) if isinstance(value, dict) else getattr(value, {}, None)\n    return {tags}.get(kind, {unknown_tag}) if isinstance(kind, str) else {unknown_tag}\n\n\n{name} = Annotated[Union[{}], Discriminator({func})]\n",
        entries.join(", "),
        py_str(property),
        py_str(&ident),
        variants.join(", "),
    )
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
            raw: None,
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
                    let default = f.default.as_ref().map(py_value);
                    let assign = match (alias, &default, f.required) {
                        (Some(a), Some(d), false) => Some(format!("Field({d}, alias={a})")),
                        (Some(a), None, false) => Some(format!("Field(None, alias={a})")),
                        (Some(a), _, true) => Some(format!("Field(alias={a})")),
                        (None, Some(d), false) => Some(d.clone()),
                        (None, None, false) => Some("None".into()),
                        (None, _, true) => None,
                    };
                    // A field with a default is never null, so `quantity: int = 1`.
                    let ann = if default.is_some() && !f.required {
                        render(&f.ty, &mut uses, Target::Sdk)
                    } else {
                        ann
                    };
                    item.fields.push(FieldItem {
                        ident: f.ident.clone(),
                        annotation: ann,
                        assign,
                        doc: f.description.as_deref().map(|d| d.trim().to_string()),
                    });
                }
            }
            Shape::Enum { values } => item.expr = Some(crate::py::enum_type(values, &mut uses)),
            Shape::Union {
                variants,
                discriminator,
            } => {
                let named: Vec<&str> = variants
                    .iter()
                    .filter_map(|v| match v {
                        TypeRef::Named { name } => Some(name.as_str()),
                        _ => None,
                    })
                    .collect();
                match discriminator {
                    Some(d) if named.len() == variants.len() => {
                        pydantic.extend(["Discriminator", "Tag"]);
                        let cases: Vec<_> = d
                            .mapping
                            .iter()
                            .map(|c| (c.value.clone(), c.type_name.clone()))
                            .collect();
                        item.raw = Some(tagged_union(
                            &t.name,
                            &d.property,
                            &cases,
                            &t.name,
                            &mut uses,
                        ));
                        for (_, ty) in &cases {
                            uses.named.insert(ty.clone());
                        }
                    }
                    _ => {
                        item.expr = Some(render(
                            &TypeRef::Union {
                                variants: variants.clone(),
                            },
                            &mut uses,
                            Target::Sdk,
                        ))
                    }
                }
            }
            Shape::Alias { target } => item.expr = Some(render(target, &mut uses, Target::Sdk)),
        }
        items.push(item);
    }
    if api
        .webhooks
        .iter()
        .any(|w| matches!(w.payload, TypeRef::Named { .. }))
    {
        pydantic.extend(["Discriminator", "Tag"]);
        let cases: Vec<_> = api
            .webhooks
            .iter()
            .filter_map(|w| match &w.payload {
                TypeRef::Named { name } => Some((w.name.clone(), name.clone())),
                _ => None,
            })
            .collect();
        items.push(TypeItem {
            name: "WebhookEvent".into(),
            doc: None,
            expr: None,
            raw: Some(tagged_union(
                "WebhookEvent",
                "type",
                &cases,
                "WebhookEvent",
                &mut uses,
            )),
            config: String::new(),
            fields: vec![],
        });
    }
    pydantic.sort();
    pydantic.dedup();
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
fn sorted_types(api: &Api) -> Vec<&TypeDef> {
    fn visit<'a>(
        api: &'a Api,
        t: &'a TypeDef,
        seen: &mut BTreeSet<String>,
        out: &mut Vec<&'a TypeDef>,
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
    pub doc: String,
    pub imports: Vec<String>,
    pub types: Vec<String>,
    pub sync: Vec<MethodItem>,
    pub sync_raw: Vec<MethodItem>,
    pub asynchronous: Vec<MethodItem>,
    pub async_raw: Vec<MethodItem>,
}

#[derive(Serialize)]
pub struct MethodItem {
    pub name: String,
    pub is_async: bool,
    /// A positional parameter before the keyword-only ones.
    pub first: Option<String>,
    pub args: Vec<String>,
    pub returns: String,
    pub doc: String,
    pub body: Vec<String>,
}

enum Kind {
    /// `cast_to` is the Python expression for the parsed type, `None` for no body.
    Request {
        cast: String,
        accept: Option<String>,
    },
    Page {
        paging: String,
        item: String,
        response: String,
    },
    Stream {
        event: String,
    },
}

struct Plan {
    name: String,
    args: Vec<String>,
    doc: String,
    deprecated: Option<String>,
    pre: Vec<String>,
    kwargs: Vec<String>,
    http: String,
    path: String,
    kind: Kind,
}

pub fn resource_ctx(api: &Api, r: &oryva_ir::Resource) -> ResourceCtx {
    let mut uses = Uses::default();
    let mut needs_quote = false;
    let plans: Vec<Plan> = r
        .methods
        .iter()
        .map(|m| plan(api, &r.name, m, &mut uses, &mut needs_quote))
        .collect();
    let mut imports = import_lines(&uses);
    if needs_quote {
        imports.push("from urllib.parse import quote".into());
    }
    let variants = |is_async: bool, raw: bool| -> Vec<MethodItem> {
        plans
            .iter()
            .flat_map(|p| {
                let mut v = vec![variant(p, is_async, raw, false)];
                if matches!(&p.kind, Kind::Request { cast, .. } if cast == "bytes") && !raw {
                    v.push(variant(p, is_async, raw, true));
                }
                v
            })
            .collect()
    };
    ResourceCtx {
        class: pascal(&r.name),
        doc: r
            .description
            .clone()
            .unwrap_or_else(|| format!("{} endpoints.", humanize(&r.name))),
        imports,
        types: uses.named.into_iter().collect(),
        sync: variants(false, false),
        sync_raw: variants(false, true),
        asynchronous: variants(true, false),
        async_raw: variants(true, true),
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

fn dict(entries: &[&Arg]) -> String {
    let parts: Vec<_> = entries
        .iter()
        .map(|a| format!("{}: {}", py_str(a.wire), a.ident))
        .collect();
    format!("{{{}}}", parts.join(", "))
}

fn error_class(status: u16) -> Option<&'static str> {
    Some(match status {
        400 => "BadRequestError",
        401 => "AuthenticationError",
        403 => "PermissionDeniedError",
        404 => "NotFoundError",
        409 => "ConflictError",
        422 => "UnprocessableEntityError",
        429 => "RateLimitError",
        500..=599 => "InternalServerError",
        _ => return None,
    })
}

fn one_line(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The items type of a page: follow `path` through the response object's fields.
fn item_type(api: &Api, ty: &TypeRef, path: &[String]) -> Option<TypeRef> {
    match path.split_first() {
        None => match ty {
            TypeRef::Array { items } => Some((**items).clone()),
            TypeRef::Nullable { inner } => item_type(api, inner, path),
            _ => None,
        },
        Some((head, rest)) => match ty {
            TypeRef::Named { name } => match api.type_def(name).map(|t| &t.shape) {
                Some(Shape::Object { fields }) => fields
                    .iter()
                    .find(|f| &f.name == head)
                    .and_then(|f| item_type(api, &f.ty, rest)),
                Some(Shape::Alias { target }) => item_type(api, target, path),
                _ => None,
            },
            TypeRef::Nullable { inner } => item_type(api, inner, path),
            _ => None,
        },
    }
}

fn plan(api: &Api, resource: &str, m: &Method, uses: &mut Uses, needs_quote: &mut bool) -> Plan {
    let args = method_args(api, m);
    let decls: Vec<String> = args
        .iter()
        .map(|a| arg_decl(a, uses, Target::Sdk))
        .collect();

    let mut doc = description(m);
    let documented: Vec<String> = args
        .iter()
        .filter_map(|a| {
            let default = a.default.map(|d| format!("Defaults to {}.", py_value(d)));
            let text = match (a.description.map(one_line), default) {
                (Some(d), Some(def)) => format!("{d} {def}"),
                (Some(t), None) | (None, Some(t)) => t,
                (None, None) => return None,
            };
            Some(format!("    {}: {text}", a.ident))
        })
        .collect();
    if !documented.is_empty() {
        doc.push_str(&format!("\n\nArgs:\n{}", documented.join("\n")));
    }
    let mut raises: Vec<String> = Vec::new();
    for e in &m.errors {
        if let Some(class) = error_class(e.status)
            && !raises
                .iter()
                .any(|r| r.starts_with(&format!("    {class}:")))
        {
            raises.push(format!(
                "    {class}: {}",
                e.description
                    .as_deref()
                    .map(one_line)
                    .unwrap_or_else(|| format!("The API returned {}.", e.status))
            ));
        }
    }
    if !raises.is_empty() {
        doc.push_str(&format!("\n\nRaises:\n{}", raises.join("\n")));
    }
    if m.pagination.is_some() {
        doc.push_str("\n\nReturns one page. Iterate it to walk every item across pages.");
    }

    let mut pre: Vec<String> = Vec::new();
    let mut kwargs: Vec<String> = Vec::new();

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
    let path_expr = if path == m.path {
        py_str(&path)
    } else {
        pre.push(format!("path = f{}", py_str(&path)));
        "path".to_string()
    };

    let query: Vec<&Arg> = args.iter().filter(|a| a.place == Place::Query).collect();
    if !query.is_empty() {
        kwargs.push(format!("params={}", dict(&query)));
    }
    let header: Vec<&Arg> = args.iter().filter(|a| a.place == Place::Header).collect();
    if !header.is_empty() {
        kwargs.push(format!("headers={}", dict(&header)));
    }
    if let Some(b) = &m.body {
        let fields: Vec<&Arg> = args.iter().filter(|a| a.place == Place::Body).collect();
        let flat = fields.first().is_none_or(|a| !a.wire.is_empty());
        match b.encoding {
            BodyEncoding::Json | BodyEncoding::Form => {
                let kw = if b.encoding == BodyEncoding::Json {
                    "json"
                } else {
                    "data"
                };
                kwargs.push(if flat {
                    format!("{kw}={}", dict(&fields))
                } else {
                    format!("{kw}=body")
                });
            }
            BodyEncoding::Multipart => {
                let (files, data): (Vec<&Arg>, Vec<&Arg>) =
                    fields.into_iter().partition(|a| is_file(a.ty));
                if !files.is_empty() {
                    kwargs.push(format!("files={}", dict(&files)));
                }
                if !data.is_empty() {
                    kwargs.push(format!("data={}", dict(&data)));
                }
            }
            BodyEncoding::Binary => kwargs.push("content=file".into()),
        }
    }

    let kind = if let Some(p) = &m.pagination {
        let (paging, items) = match p {
            Pagination::Cursor {
                param,
                next_cursor,
                items,
            } => (
                format!(
                    "Cursor(param={}, next_cursor={}, items={})",
                    py_str(param),
                    py_tuple(next_cursor),
                    py_tuple(items)
                ),
                items,
            ),
            Pagination::Offset {
                offset_param,
                limit_param,
                items,
            } => (
                format!(
                    "Offset(offset_param={}, limit_param={}, items={})",
                    py_str(offset_param),
                    py_str(limit_param),
                    py_tuple(items)
                ),
                items,
            ),
            Pagination::Page { page_param, items } => (
                format!(
                    "Page(page_param={}, items={})",
                    py_str(page_param),
                    py_tuple(items)
                ),
                items,
            ),
        };
        let response_ty = match &m.response {
            ResponseBody::Json { ty } => ty.clone(),
            _ => TypeRef::Any,
        };
        let item = item_type(api, &response_ty, items).unwrap_or(TypeRef::Any);
        Kind::Page {
            paging,
            item: render(&item, uses, Target::Sdk),
            response: render(&response_ty, uses, Target::Sdk),
        }
    } else {
        match &m.response {
            ResponseBody::Empty => Kind::Request {
                cast: "None".into(),
                accept: None,
            },
            ResponseBody::Text => Kind::Request {
                cast: "str".into(),
                accept: Some("text/plain".into()),
            },
            ResponseBody::Binary { content_type } => Kind::Request {
                cast: "bytes".into(),
                accept: Some(content_type.clone()),
            },
            ResponseBody::Json { ty } => Kind::Request {
                cast: render(ty, uses, Target::Sdk),
                accept: None,
            },
            ResponseBody::EventStream { event } => Kind::Stream {
                event: render(event, uses, Target::Sdk),
            },
        }
    };

    let deprecated = m.deprecated.then(|| {
        let why = m
            .description
            .as_deref()
            .map(one_line)
            .unwrap_or_else(|| "it may be removed in a future version.".into());
        format!("{resource}.{}() is deprecated: {why}", m.name)
    });

    Plan {
        name: m.name.clone(),
        args: decls,
        doc,
        deprecated,
        pre,
        kwargs,
        http: py_str(&format!("{:?}", m.http_method).to_uppercase()),
        path: path_expr,
        kind,
    }
}

/// One generated method: sync or async, plain or `with_raw_response`, or (for downloads) the
/// `*_to_file` companion.
fn variant(p: &Plan, is_async: bool, raw: bool, to_file: bool) -> MethodItem {
    let (page, stream) = if is_async {
        ("AsyncPage", "AsyncStream")
    } else {
        ("SyncPage", "Stream")
    };
    let await_ = if is_async { "await " } else { "" };
    let mut body: Vec<String> = Vec::new();
    if let Some(msg) = &p.deprecated {
        body.push(format!("warn_deprecated({})", py_str(msg)));
    }
    body.extend(p.pre.iter().cloned());

    let mut call: Vec<String> = vec![p.http.clone(), p.path.clone()];
    let (helper, mut inner) = match &p.kind {
        Kind::Request { cast, accept } => {
            call.push(format!("cast_to={cast}"));
            if let Some(a) = accept {
                call.push(format!("accept={}", py_str(a)));
            }
            ("_request", cast.clone())
        }
        Kind::Page {
            paging,
            item,
            response,
        } => {
            call.push(format!("paging={paging}"));
            call.push(format!("item_type={item}"));
            call.push(format!("response_type={response}"));
            ("_paginate", format!("{page}[{item}]"))
        }
        Kind::Stream { event } => {
            call.push(format!("cast_to={event}"));
            ("_stream", format!("{stream}[{event}]"))
        }
    };
    call.extend(p.kwargs.iter().cloned());
    call.push("options=options".into());
    let (name, first, doc) = if to_file {
        call.retain(|c| !c.starts_with("cast_to="));
        call.insert(2, "dest=dest".into());
        (
            format!("{}_to_file", p.name),
            Some("dest: Union[str, os.PathLike[str]]".to_string()),
            format!(
                "{}\n\nSave the response to `dest` instead of returning the bytes. The file is written in chunks, so large downloads stay out of memory.",
                p.doc.split("\n\nArgs:").next().unwrap_or(&p.doc)
            ),
        )
    } else {
        if raw {
            call.push("raw=True".into());
            inner = format!("APIResponse[{inner}]");
        }
        (p.name.clone(), None, p.doc.clone())
    };
    let helper = if to_file { "_download" } else { helper };
    let expr = format!("self._client.{helper}({})", call.join(", "));
    if to_file {
        body.push(format!("{await_}{expr}"));
        inner = "None".into();
    } else if inner == "None" {
        body.push(format!("{await_}{expr}"));
    } else {
        body.push(format!("return {await_}{expr}"));
    }
    MethodItem {
        name,
        is_async,
        first,
        args: p.args.clone(),
        returns: inner,
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
        TypeRef::File => "open(\"sample.bin\", \"rb\")".into(),
        TypeRef::Any => "None".into(),
        TypeRef::Array { .. } => "[]".into(),
        TypeRef::Map { .. } => "{}".into(),
        TypeRef::Enum { values } => values.first().map(py_value).unwrap_or("\"...\"".into()),
        TypeRef::Nullable { inner } => example_value(api, inner),
        TypeRef::Union { variants } => variants
            .first()
            .map(|v| example_value(api, v))
            .unwrap_or("None".into()),
        TypeRef::Named { name } => match api.type_def(name).map(|t| &t.shape) {
            Some(Shape::Enum { values }) => {
                values.first().map(py_value).unwrap_or("\"...\"".into())
            }
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
    let Some((r, m)) = api.resources.iter().find_map(|r| {
        r.methods
            .iter()
            .find(|m| !matches!(m.response, ResponseBody::EventStream { .. }))
            .map(|m| (r, m))
    }) else {
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
    match (&m.response, &m.pagination) {
        (_, Some(_)) => format!("client = {class}()\nfor item in {call}:\n    print(item)"),
        (ResponseBody::Empty, _) => format!("client = {class}()\n{call}"),
        _ => format!("client = {class}()\nresult = {call}\nprint(result)"),
    }
}
