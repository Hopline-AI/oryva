//! Everything the templates need, computed up front so they only lay text out.

use std::collections::BTreeSet;

use oryva_ir::{
    Api, ApiKeyLocation, Auth, Body, BodyEncoding, HttpMethod, Method, Pagination, Param,
    ParamLocation, Resource, ResponseBody, Shape, TypeRef, WebhookSigning,
};
use serde::Serialize;

use crate::Options;
use crate::ts;

/// Classes `errors.ts` exports besides the API's own `<Name>Error`.
const ERROR_CLASSES: &[&str] = &[
    "APIError",
    "BadRequestError",
    "AuthenticationError",
    "PermissionDeniedError",
    "NotFoundError",
    "ConflictError",
    "UnprocessableEntityError",
    "RateLimitError",
    "InternalServerError",
    "APIConnectionError",
    "APIConnectionTimeoutError",
    "APIUserAbortError",
    "WebhookVerificationError",
];

#[derive(Serialize)]
pub struct Common {
    pub name: String,
    pub client: String,
    pub error: String,
    pub package: String,
    pub version: String,
    pub title: String,
    pub description: String,
    pub log_env: String,
    pub base_url_env: String,
    pub idempotency: Option<String>,
    pub auth_hint: String,
    pub environments: Vec<Env>,
    pub default_environment: String,
    pub options: Vec<AuthOption>,
    pub schemes: Vec<Scheme>,
    pub oauth: Option<OAuth>,
    pub sensitive_headers: Vec<String>,
    pub sensitive_query: Vec<String>,
    pub webhook_secret_env: Option<String>,
    pub resources: Vec<ResourceRef>,
    pub basic: bool,
    pub example_option: String,
    pub example_env: String,
    pub class_doc: String,
    /// Error classes that a model of the same name would otherwise collide with in the barrel.
    pub error_clashes: Vec<String>,
}

#[derive(Serialize)]
pub struct Env {
    pub name: String,
    pub key: String,
    pub url: String,
}

#[derive(Serialize)]
pub struct AuthOption {
    pub name: String,
    pub env: String,
}

#[derive(Serialize)]
pub struct Scheme {
    pub condition: String,
    pub apply: String,
}

#[derive(Serialize)]
pub struct OAuth {
    pub token_url: String,
    pub token_url_env: String,
    pub scopes: String,
    pub id_option: String,
    pub secret_option: String,
}

#[derive(Serialize)]
pub struct ResourceRef {
    pub name: String,
    pub class: String,
    pub file: String,
    pub params: Vec<String>,
}

/// `LEDGER_CLIENT_ID` -> `clientId`.
fn option_name(env: &str, prefix: &str) -> String {
    let stem = env.strip_prefix(&format!("{prefix}_")).unwrap_or(env);
    ts::camel(&stem.to_lowercase())
}

pub fn common(api: &Api, opts: &Options) -> Common {
    let prefix = api.name.to_uppercase();
    let mut options: Vec<AuthOption> = Vec::new();
    let mut add = |env: &str| -> String {
        let name = option_name(env, &prefix);
        if !options.iter().any(|o| o.name == name) {
            options.push(AuthOption {
                name: name.clone(),
                env: env.to_string(),
            });
        }
        name
    };
    let mut schemes = Vec::new();
    let mut oauth = None;
    let mut hints = Vec::new();
    let (mut sensitive_headers, mut sensitive_query) = (Vec::new(), Vec::new());
    for auth in &api.auth {
        match auth {
            Auth::Bearer { env_var } => {
                let o = add(env_var);
                schemes.push(Scheme {
                    condition: format!("this.{o}"),
                    apply: format!("request.headers.set('authorization', `Bearer ${{this.{o}}}`);"),
                });
                hints.push(format!("set {env_var}"));
            }
            Auth::ApiKey {
                location,
                name,
                env_var,
            } => {
                let o = add(env_var);
                let apply = match location {
                    ApiKeyLocation::Header => {
                        sensitive_headers.push(name.clone());
                        format!("request.headers.set({}, this.{o});", ts::string(name))
                    }
                    ApiKeyLocation::Query => {
                        sensitive_query.push(name.clone());
                        format!("request.query[{}] = this.{o};", ts::string(name))
                    }
                    ApiKeyLocation::Cookie => {
                        format!("request.headers.append('cookie', `{name}=${{this.{o}}}`);")
                    }
                };
                schemes.push(Scheme {
                    condition: format!("this.{o}"),
                    apply,
                });
                hints.push(format!("set {env_var}"));
            }
            Auth::Basic {
                username_env,
                password_env,
            } => {
                let (u, p) = (add(username_env), add(password_env));
                schemes.push(Scheme {
                    condition: format!("this.{u} && this.{p}"),
                    apply: format!(
                        "request.headers.set('authorization', `Basic ${{base64(`${{this.{u}}}:${{this.{p}}}`)}}`);"
                    ),
                });
                hints.push(format!("set {username_env} and {password_env}"));
            }
            Auth::OAuth2ClientCredentials {
                token_url,
                scopes,
                client_id_env,
                client_secret_env,
            } => {
                let (id, secret) = (add(client_id_env), add(client_secret_env));
                schemes.push(Scheme {
                    condition: "this.oauth".into(),
                    apply:
                        "request.headers.set('authorization', `Bearer ${await this.oauth.getToken()}`);"
                            .into(),
                });
                hints.push(format!("set {client_id_env} and {client_secret_env}"));
                oauth = Some(OAuth {
                    token_url: token_url.clone(),
                    token_url_env: format!("{prefix}_OAUTH_TOKEN_URL"),
                    scopes: scopes
                        .iter()
                        .map(|s| ts::string(s))
                        .collect::<Vec<_>>()
                        .join(", "),
                    id_option: id,
                    secret_option: secret,
                });
            }
        }
    }
    let webhook_secret_env = match &api.webhook_signing {
        Some(WebhookSigning::StandardWebhooks { secret_env }) if !api.webhooks.is_empty() => {
            Some(secret_env.clone())
        }
        _ => None,
    };
    let default_environment = api
        .servers
        .first()
        .map(|s| s.name.clone())
        .unwrap_or_default();
    let example = options.first();
    let error = format!("{}Error", ts::pascal(&api.name));
    let error_clashes = ERROR_CLASSES
        .iter()
        .map(|n| n.to_string())
        .chain([error.clone()])
        .filter(|n| api.type_def(n).is_some())
        .collect();
    Common {
        error_clashes,
        name: api.name.clone(),
        client: ts::pascal(&api.name),
        error,
        package: opts.package.clone(),
        version: opts.version.clone(),
        title: api.title.clone(),
        description: api.description.clone().unwrap_or_default(),
        log_env: format!("{prefix}_LOG"),
        base_url_env: format!("{prefix}_BASE_URL"),
        idempotency: api.idempotency_header.clone(),
        auth_hint: hints.join(", or "),
        environments: api
            .servers
            .iter()
            .map(|s| Env {
                name: s.name.clone(),
                key: ts::prop(&s.name),
                url: s.url.clone(),
            })
            .collect(),
        default_environment,
        schemes,
        oauth,
        sensitive_headers,
        sensitive_query,
        webhook_secret_env,
        resources: api
            .resources
            .iter()
            .map(|r| ResourceRef {
                name: ts::prop(&r.name),
                class: resource_class(api, &r.name),
                file: r.name.clone(),
                params: r
                    .methods
                    .iter()
                    .filter(|m| has_params(m))
                    .map(|m| params_name(r, m))
                    .collect(),
            })
            .collect(),
        basic: api.auth.iter().any(|a| matches!(a, Auth::Basic { .. })),
        example_option: example.map(|o| o.name.clone()).unwrap_or_default(),
        example_env: example.map(|o| o.env.clone()).unwrap_or_default(),
        class_doc: ts::doc(&[
            api.title.clone(),
            api.description.clone().unwrap_or_default(),
            format!(
                "```ts\nconst client = new {}();\n```",
                ts::pascal(&api.name)
            ),
        ]),
        options,
    }
}

pub fn types_source(api: &Api) -> String {
    api.types
        .iter()
        .map(ts::type_def)
        .collect::<Vec<_>>()
        .join("\n")
}

#[derive(Serialize)]
pub struct WebhooksCtx {
    pub payloads: Vec<String>,
    pub secret_env: String,
}

pub fn webhooks_ctx(api: &Api, secret_env: &str) -> WebhooksCtx {
    let mut payloads = BTreeSet::new();
    for w in &api.webhooks {
        ts::named(&w.payload, &mut payloads);
    }
    WebhooksCtx {
        payloads: payloads.into_iter().collect(),
        secret_env: secret_env.to_string(),
    }
}

#[derive(Serialize)]
pub struct ResourceCtx {
    pub class: String,
    pub description: String,
    pub methods: Vec<String>,
    pub declarations: Vec<String>,
    pub types: Vec<String>,
    pub uploadable: bool,
    pub paginated: bool,
    pub streams: bool,
    pub deprecated: bool,
    pub helpers: Vec<String>,
    pub doc: String,
}

fn params_name(r: &Resource, m: &Method) -> String {
    format!("{}{}Params", ts::pascal(&r.name), ts::pascal(&m.name))
}

/// The class for a resource. A component schema of the same name wins the plain name.
fn resource_class(api: &Api, name: &str) -> String {
    let class = ts::pascal(name);
    if api.type_def(&class).is_some() {
        format!("{class}Resource")
    } else {
        class
    }
}

fn is_extra(p: &Param) -> bool {
    p.location != ParamLocation::Path
}

fn has_params(m: &Method) -> bool {
    m.params.iter().any(is_extra) || m.body.is_some()
}

/// The body as an object whose fields join the params, or `None` when it needs its own field.
fn object_body<'a>(api: &Api, body: &'a Body) -> Option<&'a str> {
    if body.encoding == BodyEncoding::Binary {
        return None;
    }
    match &body.ty {
        TypeRef::Named { name } => match api.type_def(name).map(|t| &t.shape) {
            Some(Shape::Object { .. }) => Some(name),
            _ => None,
        },
        _ => None,
    }
}

fn list(names: &[&str]) -> String {
    let items: Vec<String> = names.iter().map(|n| ts::string(n)).collect();
    format!("[{}]", items.join(", "))
}

pub fn resource_ctx(api: &Api, r: &Resource) -> ResourceCtx {
    let mut ctx = ResourceCtx {
        class: resource_class(api, &r.name),
        description: r.description.clone().unwrap_or_default(),
        methods: Vec::new(),
        declarations: Vec::new(),
        types: Vec::new(),
        uploadable: false,
        paginated: false,
        streams: false,
        deprecated: false,
        helpers: Vec::new(),
        doc: ts::doc(&[r.description.clone().unwrap_or_default()]),
    };
    let mut names = BTreeSet::new();
    let mut needs_pick = false;
    let mut needs_omit = false;
    for m in &r.methods {
        let method = method_source(api, r, m, &mut names);
        needs_pick |= method.contains("pick(");
        needs_omit |= method.contains("omit(");
        ctx.paginated |= m.pagination.is_some();
        ctx.streams |= matches!(m.response, ResponseBody::EventStream { .. });
        ctx.deprecated |= m.deprecated;
        if has_params(m) {
            ctx.declarations.push(params_decl(api, r, m, &mut names));
        }
        ctx.methods.push(method);
    }
    ctx.uploadable = ctx
        .methods
        .iter()
        .chain(&ctx.declarations)
        .any(|s| s.contains("Uploadable"));
    ctx.types = names
        .into_iter()
        .filter(|n| api.type_def(n).is_some())
        .collect();
    if needs_pick {
        ctx.helpers.push("pick".into());
    }
    if needs_omit {
        ctx.helpers.push("omit".into());
    }
    ctx
}

fn path_params(m: &Method) -> Vec<&Param> {
    m.params
        .iter()
        .filter(|p| p.location == ParamLocation::Path)
        .collect()
}

fn extras(m: &Method, location: ParamLocation) -> Vec<&Param> {
    m.params.iter().filter(|p| p.location == location).collect()
}

fn params_decl(api: &Api, r: &Resource, m: &Method, names: &mut BTreeSet<String>) -> String {
    let name = params_name(r, m);
    let extra_fields: String = m
        .params
        .iter()
        .filter(|p| is_extra(p))
        .map(|p| {
            ts::field(&oryva_ir::Field {
                name: p.name.clone(),
                ident: p.ident.clone(),
                ty: p.ty.clone(),
                required: p.required,
                description: p.description.clone(),
                default: p.default.clone(),
            })
        })
        .collect();
    m.params.iter().for_each(|p| ts::named(&p.ty, names));
    let Some(body) = &m.body else {
        return format!("export interface {name} {{\n{extra_fields}}}\n");
    };
    ts::named(&body.ty, names);
    match object_body(api, body) {
        Some(body_name) if extra_fields.is_empty() => {
            format!("export type {name} = {body_name};\n")
        }
        Some(body_name) => format!("export type {name} = {body_name} & {{\n{extra_fields}}};\n"),
        None => format!(
            "export interface {name} {{\n{}{}: {};\n{extra_fields}}}\n",
            ts::doc(&["The request body.".to_string()]),
            if body.required { "body" } else { "body?" },
            ts::ty(&body.ty)
        ),
    }
}

fn response_type(m: &Method, names: &mut BTreeSet<String>) -> String {
    match &m.response {
        ResponseBody::Empty => "void".into(),
        ResponseBody::Json { ty } => {
            ts::named(ty, names);
            ts::ty(ty)
        }
        ResponseBody::Binary { .. } => "Response".into(),
        ResponseBody::Text => "string".into(),
        ResponseBody::EventStream { event } => {
            ts::named(event, names);
            format!("Stream<{}>", ts::ty(event))
        }
    }
}

/// The type of one page's items: follow `items` from the response type.
fn item_type(api: &Api, m: &Method, items: &[String]) -> TypeRef {
    let ResponseBody::Json { ty } = &m.response else {
        return TypeRef::Any;
    };
    let mut current = ty.clone();
    for key in items {
        let TypeRef::Named { name } = &current else {
            return TypeRef::Any;
        };
        let Some(Shape::Object { fields }) = api.type_def(name).map(|t| &t.shape) else {
            return TypeRef::Any;
        };
        let Some(field) = fields.iter().find(|f| &f.name == key) else {
            return TypeRef::Any;
        };
        current = field.ty.clone();
    }
    match current {
        TypeRef::Array { items } => *items,
        _ => TypeRef::Any,
    }
}

fn pagination_literal(p: &Pagination) -> String {
    let path = |v: &Vec<String>| list(&v.iter().map(String::as_str).collect::<Vec<_>>());
    match p {
        Pagination::Cursor {
            param,
            next_cursor,
            items,
        } => format!(
            "{{ style: 'cursor', param: {}, nextCursor: {}, items: {} }}",
            ts::string(param),
            path(next_cursor),
            path(items)
        ),
        Pagination::Offset {
            offset_param,
            limit_param,
            items,
        } => format!(
            "{{ style: 'offset', offsetParam: {}, limitParam: {}, items: {} }}",
            ts::string(offset_param),
            ts::string(limit_param),
            path(items)
        ),
        Pagination::Page { page_param, items } => format!(
            "{{ style: 'page', pageParam: {}, items: {} }}",
            ts::string(page_param),
            path(items)
        ),
    }
}

fn pagination_items(p: &Pagination) -> &Vec<String> {
    match p {
        Pagination::Cursor { items, .. }
        | Pagination::Offset { items, .. }
        | Pagination::Page { items, .. } => items,
    }
}

/// The JSDoc, signature and body of one method.
fn method_source(api: &Api, r: &Resource, m: &Method, names: &mut BTreeSet<String>) -> String {
    let verb = match m.http_method {
        HttpMethod::Get => "GET",
        HttpMethod::Post => "POST",
        HttpMethod::Put => "PUT",
        HttpMethod::Patch => "PATCH",
        HttpMethod::Delete => "DELETE",
    };

    let description = if m.deprecated {
        None
    } else {
        m.description.clone()
    };
    let mut docs = vec![
        m.summary.clone().unwrap_or_default(),
        description.unwrap_or_default(),
    ];
    if let Some(hint) = &m.agent.hint {
        docs.push(format!("Agent hint: {hint}"));
    }
    if m.deprecated {
        docs.push(format!(
            "@deprecated {}",
            m.description
                .as_deref()
                .unwrap_or("This method will be removed.")
        ));
    }

    // Arguments: path parameters first, then one params object, then request options.
    let mut args = Vec::new();
    let mut path = ts::string(&m.path);
    let mut uses_template = false;
    for p in path_params(m) {
        let var = ts::variable(&p.ident);
        ts::named(&p.ty, names);
        args.push(format!("{var}: {}", ts::ty(&p.ty)));
        path = path.replace(
            &format!("{{{}}}", p.name),
            &format!("${{encodeURIComponent({var})}}"),
        );
        uses_template = true;
    }
    if uses_template {
        path = format!("`{}`", &path[1..path.len() - 1]);
    }

    let queries: Vec<&str> = extras(m, ParamLocation::Query)
        .iter()
        .map(|p| p.name.as_str())
        .collect();
    let headers: Vec<&str> = extras(m, ParamLocation::Header)
        .iter()
        .map(|p| p.name.as_str())
        .collect();
    let mut fields = vec![format!("method: '{verb}'"), format!("path: {path}")];
    if has_params(m) {
        // `params = {}` only type-checks when no key of the params type is required, and an
        // optional body of an object type still brings that object's required fields.
        let body_needs_fields = m.body.as_ref().is_some_and(|b| {
            b.required
                || object_body(api, b)
                    .and_then(|n| api.type_def(n))
                    .is_some_and(|t| matches!(&t.shape, Shape::Object { fields } if fields.iter().any(|f| f.required)))
        });
        let required = body_needs_fields || m.params.iter().any(|p| is_extra(p) && p.required);
        let ptype = params_name(r, m);
        args.push(if required {
            format!("params: {ptype}")
        } else {
            format!("params: {ptype} = {{}}")
        });
        let body_field = m.body.as_ref().map(|b| (b, object_body(api, b).is_some()));
        let all: Vec<&str> = queries.iter().chain(&headers).copied().collect();
        if !queries.is_empty() {
            fields.push(if body_field.is_none() && headers.is_empty() {
                "query: params".into()
            } else {
                format!("query: pick(params, {})", list(&queries))
            });
        }
        if !headers.is_empty() {
            fields.push(format!("headers: pick(params, {})", list(&headers)));
        }
        match body_field {
            Some((_, true)) if all.is_empty() => fields.push("body: params".into()),
            Some((_, true)) => fields.push(format!("body: omit(params, {})", list(&all))),
            Some((_, false)) => fields.push("body: params.body".into()),
            None => {}
        }
        if let Some(body) = &m.body {
            match body.encoding {
                BodyEncoding::Json => {}
                BodyEncoding::Multipart => fields.push("encoding: 'multipart'".into()),
                BodyEncoding::Form => fields.push("encoding: 'form'".into()),
                BodyEncoding::Binary => {
                    fields.push("encoding: 'binary'".into());
                    if let Some(ct) = &body.content_type {
                        fields.push(format!("contentType: {}", ts::string(ct)));
                    }
                }
            }
        }
    }
    args.push("options?: RequestOptions".into());

    let ret = response_type(m, names);
    let response_kind = match &m.response {
        ResponseBody::Json { .. } => None,
        ResponseBody::Empty => Some("empty"),
        ResponseBody::Binary { .. } => Some("binary"),
        ResponseBody::Text => Some("text"),
        ResponseBody::EventStream { .. } => Some("stream"),
    };
    if let Some(kind) = response_kind {
        fields.push(format!("response: '{kind}'"));
    }

    let (ret_type, call) = match &m.pagination {
        Some(p) => {
            let item_ty = item_type(api, m, pagination_items(p));
            ts::named(&item_ty, names);
            let item = ts::ty(&item_ty);
            fields.push(format!("pagination: {}", pagination_literal(p)));
            (
                format!("PagePromise<{item}, {ret}>"),
                format!("this.client.requestPage<{item}, {ret}>"),
            )
        }
        None => (
            format!("APIPromise<{ret}>"),
            format!("this.client.request<{ret}>"),
        ),
    };
    let warn = if m.deprecated {
        format!(
            "warnDeprecated({}, {});\n",
            ts::string(&format!("{}.{}", r.name, ts::camel(&m.name))),
            m.description
                .as_deref()
                .map(ts::string)
                .unwrap_or_else(|| "undefined".into())
        )
    } else {
        String::new()
    };

    format!(
        "{}{}({}): {ret_type} {{\n{warn}return {call}({{ {} }}, options);\n}}\n",
        ts::doc(&docs),
        ts::camel(&m.name),
        args.join(", "),
        fields.join(", ")
    )
}
