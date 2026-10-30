//! The API model every emitter reads.
//!
//! Spec parsers (OpenAPI today, `stainless.yml` later) fill it in; emitters never see the
//! original spec. Names arrive normalized: `ident` and resource/method names are snake_case,
//! type names are PascalCase, and every inline object schema has been hoisted into a named
//! [`TypeDef`].

use serde::{Deserialize, Serialize};

/// Bumped on any breaking change to the shapes below.
pub const IR_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Api {
    pub ir_version: u32,
    /// snake_case stem for package and module names, e.g. `sarvam`.
    pub name: String,
    pub title: String,
    pub version: String,
    pub description: Option<String>,
    pub base_url: Option<String>,
    pub auth: Option<Auth>,
    pub resources: Vec<Resource>,
    pub types: Vec<TypeDef>,
}

impl Api {
    pub fn type_def(&self, name: &str) -> Option<&TypeDef> {
        self.types.iter().find(|t| t.name == name)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Auth {
    /// A key sent in a request header, e.g. `api-subscription-key`.
    ApiKey { header: String, env_var: String },
    /// `Authorization: Bearer <token>`.
    Bearer { env_var: String },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Resource {
    /// e.g. `text` in `client.text.translate(...)`.
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
    pub ty: TypeRef,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BodyEncoding {
    Json,
    Multipart,
    Form,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ResponseBody {
    Empty,
    Json { ty: TypeRef },
    Binary { content_type: String },
    Text,
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
    pub examples: Vec<serde_json::Value>,
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
    /// Binary upload, e.g. a multipart file part.
    File,
    Any,
    Array {
        items: Box<TypeRef>,
    },
    Map {
        values: Box<TypeRef>,
    },
    /// Inline string enum. Named enums are `TypeDef`s with `Shape::Enum`.
    Enum {
        values: Vec<String>,
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
    Enum {
        values: Vec<String>,
    },
    Union {
        variants: Vec<TypeRef>,
        discriminator: Option<String>,
    },
    Alias {
        target: TypeRef,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Field {
    /// Name on the wire.
    pub name: String,
    pub ident: String,
    pub ty: TypeRef,
    pub required: bool,
    pub description: Option<String>,
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
            base_url: Some("https://api.example.com".into()),
            auth: Some(Auth::ApiKey {
                header: "x-key".into(),
                env_var: "DEMO_API_KEY".into(),
            }),
            resources: vec![Resource {
                name: "items".into(),
                description: None,
                methods: vec![Method {
                    name: "delete".into(),
                    http_method: HttpMethod::Delete,
                    path: "/items/{item_id}".into(),
                    summary: None,
                    description: None,
                    params: vec![Param {
                        name: "item_id".into(),
                        ident: "item_id".into(),
                        location: ParamLocation::Path,
                        required: true,
                        ty: TypeRef::String { format: None },
                        description: None,
                    }],
                    body: None,
                    response: ResponseBody::Empty,
                    errors: vec![],
                    deprecated: false,
                    agent: AgentNotes {
                        destructive: true,
                        idempotent: true,
                        ..Default::default()
                    },
                }],
            }],
            types: vec![],
        };
        let json = serde_json::to_string(&api).unwrap();
        assert_eq!(serde_json::from_str::<Api>(&json).unwrap(), api);
        assert!(json.contains(r#""http_method":"DELETE""#));
    }
}
