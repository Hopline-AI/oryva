//! The API model every emitter reads.
//!
//! Spec parsers (OpenAPI today, `stainless.yml` later) fill it in; emitters never see the
//! original spec. Names arrive normalized: `ident` and resource/method names are snake_case,
//! type names are PascalCase, and every inline object schema has been hoisted into a named
//! [`TypeDef`].

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Bumped on any breaking change to the shapes below.
pub const IR_VERSION: u32 = 2;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Api {
    pub ir_version: u32,
    /// snake_case stem for package and module names, e.g. `ledger`.
    pub name: String,
    pub title: String,
    pub version: String,
    pub description: Option<String>,
    /// The first server is the default environment.
    pub servers: Vec<Server>,
    /// Schemes the API accepts. A client needs credentials for any one of them.
    pub auth: Vec<Auth>,
    /// Header that carries an idempotency key. Clients send a fresh key with every non-GET
    /// request and reuse it across retries of that request.
    pub idempotency_header: Option<String>,
    pub resources: Vec<Resource>,
    pub types: Vec<TypeDef>,
    /// Events the API delivers to the customer's endpoint (OpenAPI 3.1 `webhooks`).
    pub webhooks: Vec<Webhook>,
    pub webhook_signing: Option<WebhookSigning>,
}

impl Api {
    pub fn type_def(&self, name: &str) -> Option<&TypeDef> {
        self.types.iter().find(|t| t.name == name)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Server {
    /// snake_case environment name, e.g. `production`, `sandbox`.
    pub name: String,
    pub url: String,
    pub description: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Auth {
    ApiKey {
        location: ApiKeyLocation,
        /// Header, query parameter or cookie name, e.g. `x-api-key`.
        name: String,
        env_var: String,
    },
    /// `Authorization: Bearer <token>`.
    Bearer { env_var: String },
    /// `Authorization: Basic <base64(username:password)>`.
    Basic {
        username_env: String,
        password_env: String,
    },
    /// The client exchanges its id and secret for a token, caches it until shortly before
    /// expiry, and sends it as a bearer token.
    #[serde(rename = "oauth2_client_credentials")]
    OAuth2ClientCredentials {
        token_url: String,
        scopes: Vec<String>,
        client_id_env: String,
        client_secret_env: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApiKeyLocation {
    Header,
    Query,
    Cookie,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Resource {
    /// e.g. `customers` in `client.customers.list(...)`.
    pub name: String,
    pub description: Option<String>,
    pub methods: Vec<Method>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Method {
    /// Unique within its resource.
    pub name: String,
    pub http_method: HttpMethod,
    /// Path template as written in the spec, e.g. `/items/{item_id}`.
    pub path: String,
    pub summary: Option<String>,
    pub description: Option<String>,
    pub params: Vec<Param>,
    pub body: Option<Body>,
    pub response: ResponseBody,
    /// Set when the method returns one page of a list; emitters add auto-paging on top.
    pub pagination: Option<Pagination>,
    pub errors: Vec<ErrorCase>,
    pub deprecated: bool,
    pub agent: AgentNotes,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum HttpMethod {
    Get,
    Post,
    Put,
    Patch,
    Delete,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Param {
    /// Name on the wire.
    pub name: String,
    pub ident: String,
    pub location: ParamLocation,
    pub required: bool,
    pub ty: TypeRef,
    pub description: Option<String>,
    pub default: Option<Value>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ParamLocation {
    Path,
    Query,
    Header,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Body {
    pub encoding: BodyEncoding,
    pub required: bool,
    /// Usually a `Named` object; emitters flatten its fields into keyword arguments.
    /// `File` for a `Binary` body.
    pub ty: TypeRef,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BodyEncoding {
    Json,
    Multipart,
    Form,
    /// Raw bytes, e.g. `application/octet-stream`.
    Binary,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ResponseBody {
    Empty,
    Json {
        ty: TypeRef,
    },
    Binary {
        content_type: String,
    },
    Text,
    /// `text/event-stream`: each event's `data` is JSON of type `event`.
    EventStream {
        event: TypeRef,
    },
}

/// How to fetch the next page. Paths are field names from the response root, e.g. `["data"]`.
/// Auto-paging stops when a page has no items, or when `next_cursor` is missing or null.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Pagination {
    /// Send the previous page's `next_cursor` back as query parameter `param`.
    Cursor {
        param: String,
        next_cursor: Vec<String>,
        items: Vec<String>,
    },
    /// Advance query parameter `offset_param` by the number of items received.
    Offset {
        offset_param: String,
        limit_param: String,
        items: Vec<String>,
    },
    /// Increment query parameter `page_param`, starting from the value the caller passed (or 1).
    Page {
        page_param: String,
        items: Vec<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ErrorCase {
    pub status: u16,
    pub description: Option<String>,
    pub ty: Option<TypeRef>,
}

/// What an agent needs beyond the schema. Defaults come from the HTTP method; the
/// `x-oryva-agent` operation extension overrides them.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct AgentNotes {
    pub read_only: bool,
    pub destructive: bool,
    pub idempotent: bool,
    /// When to use the call, real limits, gotchas. Copied into tool descriptions verbatim.
    pub hint: Option<String>,
    pub examples: Vec<Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Webhook {
    /// Event name as delivered, e.g. `invoice.paid`.
    pub name: String,
    /// snake_case identifier, e.g. `invoice_paid`.
    pub ident: String,
    pub description: Option<String>,
    pub payload: TypeRef,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WebhookSigning {
    /// The Standard Webhooks scheme (standardwebhooks.com): `webhook-id`, `webhook-timestamp`
    /// and `webhook-signature` headers, HMAC-SHA256 over `id.timestamp.body`, base64-encoded,
    /// secret prefixed `whsec_`.
    StandardWebhooks { secret_env: String },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TypeRef {
    String {
        format: Option<String>,
    },
    Integer,
    Number,
    Boolean,
    /// Binary upload or download.
    File,
    Any,
    Array {
        items: Box<TypeRef>,
    },
    Map {
        values: Box<TypeRef>,
    },
    /// Inline enum of strings or integers. Named enums are `TypeDef`s with `Shape::Enum`.
    /// Generated code accepts values outside the list, so new server values don't break clients.
    Enum {
        values: Vec<Value>,
    },
    Named {
        name: String,
    },
    Nullable {
        inner: Box<TypeRef>,
    },
    Union {
        variants: Vec<TypeRef>,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TypeDef {
    pub name: String,
    pub description: Option<String>,
    pub shape: Shape,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Shape {
    Object {
        fields: Vec<Field>,
    },
    /// Strings or integers.
    Enum {
        values: Vec<Value>,
    },
    Union {
        variants: Vec<TypeRef>,
        discriminator: Option<Discriminator>,
    },
    Alias {
        target: TypeRef,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Discriminator {
    /// Wire name of the property that selects the variant, e.g. `type`.
    pub property: String,
    /// Property value → variant type name, in spec order.
    pub mapping: Vec<DiscriminatorCase>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DiscriminatorCase {
    pub value: String,
    pub type_name: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Field {
    /// Name on the wire.
    pub name: String,
    pub ident: String,
    pub ty: TypeRef,
    pub required: bool,
    pub description: Option<String>,
    pub default: Option<Value>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_through_json() {
        let api = Api {
            ir_version: IR_VERSION,
            name: "demo".into(),
            title: "Demo".into(),
            version: "1".into(),
            description: None,
            servers: vec![Server {
                name: "production".into(),
                url: "https://api.example.com".into(),
                description: None,
            }],
            auth: vec![Auth::OAuth2ClientCredentials {
                token_url: "https://api.example.com/oauth/token".into(),
                scopes: vec!["read".into()],
                client_id_env: "DEMO_CLIENT_ID".into(),
                client_secret_env: "DEMO_CLIENT_SECRET".into(),
            }],
            idempotency_header: Some("Idempotency-Key".into()),
            resources: vec![Resource {
                name: "items".into(),
                description: None,
                methods: vec![Method {
                    name: "list".into(),
                    http_method: HttpMethod::Get,
                    path: "/items".into(),
                    summary: None,
                    description: None,
                    params: vec![Param {
                        name: "limit".into(),
                        ident: "limit".into(),
                        location: ParamLocation::Query,
                        required: false,
                        ty: TypeRef::Integer,
                        description: None,
                        default: Some(20.into()),
                    }],
                    body: None,
                    response: ResponseBody::EventStream {
                        event: TypeRef::Named {
                            name: "Item".into(),
                        },
                    },
                    pagination: Some(Pagination::Cursor {
                        param: "cursor".into(),
                        next_cursor: vec!["next_cursor".into()],
                        items: vec!["data".into()],
                    }),
                    errors: vec![],
                    deprecated: false,
                    agent: AgentNotes {
                        read_only: true,
                        idempotent: true,
                        ..Default::default()
                    },
                }],
            }],
            types: vec![],
            webhooks: vec![],
            webhook_signing: Some(WebhookSigning::StandardWebhooks {
                secret_env: "DEMO_WEBHOOK_SECRET".into(),
            }),
        };
        let json = serde_json::to_string(&api).unwrap();
        assert_eq!(serde_json::from_str::<Api>(&json).unwrap(), api);
        assert!(json.contains(r#""kind":"oauth2_client_credentials""#));
        assert!(json.contains(r#""kind":"event_stream""#));
    }
}
