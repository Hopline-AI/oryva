use super::*;
use oryva_ir::{
    ApiKeyLocation, BodyEncoding, Pagination, ResponseBody, Shape, TypeRef, WebhookSigning,
};

fn spec(version: &str, rest: &str) -> String {
    format!("openapi: {version}\ninfo: {{title: Demo API, version: '1'}}\n{rest}")
}

fn ok(version: &str, rest: &str) -> Api {
    parse_str(&spec(version, rest)).unwrap()
}

fn err(version: &str, rest: &str) -> String {
    parse_str(&spec(version, rest)).unwrap_err().to_string()
}

fn string() -> TypeRef {
    TypeRef::String { format: None }
}

fn nullable(inner: TypeRef) -> TypeRef {
    TypeRef::Nullable {
        inner: Box::new(inner),
    }
}

fn field_ty(api: &Api, ty: &str, field: &str) -> TypeRef {
    let Shape::Object { fields } = &api.type_def(ty).unwrap().shape else {
        panic!("{ty} is not an object");
    };
    fields.iter().find(|f| f.name == field).unwrap().ty.clone()
}

const R200: &str = "responses: {'200': {description: ok}}";

#[test]
fn names_resources_and_methods() {
    let api = ok(
        "3.1.0",
        &format!(
            "paths:
  /pets:
    get: {{tags: [pets], operationId: pets_list, {R200}}}
    post: {{tags: [pets], {R200}}}
  /pets/{{id}}:
    get: {{tags: [pets], {R200}}}
    put: {{tags: [pets], {R200}}}
    delete: {{tags: [pets], {R200}}}
  /items:
    get: {{operationId: listAllItems, {R200}}}
  /v1/things:
    get: {{operationId: things, x-oryva-method: fetch, x-oryva-resource: Stuff, {R200}}}
"
        ),
    );
    let names: Vec<(&str, Vec<&str>)> = api
        .resources
        .iter()
        .map(|r| {
            (
                r.name.as_str(),
                r.methods.iter().map(|m| m.name.as_str()).collect(),
            )
        })
        .collect();
    assert_eq!(
        names,
        [
            ("pets", vec!["list", "create", "get", "update", "delete"]),
            ("items", vec!["list_all_items"]),
            ("stuff", vec!["fetch"]),
        ]
    );
    assert_eq!(api.name, "demo");
}

#[test]
fn colliding_methods_are_an_error_naming_both() {
    let e = err(
        "3.1.0",
        &format!(
            "paths:
  /a: {{get: {{tags: [t], operationId: t_run, {R200}}}}}
  /b: {{get: {{tags: [t], operationId: run, {R200}}}}}
"
        ),
    );
    assert!(e.contains("#/paths/~1b/get"), "{e}");
    assert!(e.contains("#/paths/~1a/get"), "{e}");
    assert!(e.contains("t.run"), "{e}");
}

#[test]
fn nullable_in_3_0_and_3_1() {
    let schemas = |t: &str| {
        format!(
            "components:\n  schemas:\n    A:\n      type: object\n      properties:\n        x: {t}\n"
        )
    };
    let v30 = ok("3.0.3", &schemas("{type: string, nullable: true}"));
    let v31 = ok("3.1.0", &schemas("{type: [string, 'null']}"));
    assert_eq!(field_ty(&v30, "A", "x"), nullable(string()));
    assert_eq!(field_ty(&v31, "A", "x"), nullable(string()));
}

#[test]
fn one_of_drops_null_variant() {
    let api = ok(
        "3.1.0",
        "components:
  schemas:
    A:
      type: object
      properties:
        one: {oneOf: [{type: string}, {type: 'null'}]}
        two: {anyOf: [{type: string}, {type: integer}, {type: 'null'}]}
        three: {oneOf: [{type: string}, {type: integer}]}
",
    );
    assert_eq!(field_ty(&api, "A", "one"), nullable(string()));
    let union = TypeRef::Union {
        variants: vec![string(), TypeRef::Integer],
    };
    assert_eq!(field_ty(&api, "A", "two"), nullable(union.clone()));
    assert_eq!(field_ty(&api, "A", "three"), union);
}

#[test]
fn all_of_merges_fields_and_required() {
    let api = ok(
        "3.0.3",
        "components:
  schemas:
    Base:
      type: object
      required: [id]
      properties:
        id: {type: string}
    Pet:
      allOf:
        - $ref: '#/components/schemas/Base'
        - type: object
          required: [name]
          properties:
            name: {type: string}
            tag: {type: string}
",
    );
    let Shape::Object { fields } = &api.type_def("Pet").unwrap().shape else {
        panic!("Pet should be an object");
    };
    let got: Vec<(&str, bool)> = fields
        .iter()
        .map(|f| (f.name.as_str(), f.required))
        .collect();
    assert_eq!(got, [("id", true), ("name", true), ("tag", false)]);
}

#[test]
fn external_ref_is_an_error() {
    let e = err(
        "3.1.0",
        "paths:
  /a:
    get:
      responses:
        '200':
          description: ok
          content:
            application/json:
              schema: {$ref: 'other.yaml#/Thing'}
",
    );
    assert!(
        e.contains("#/paths/~1a/get/responses/200/content/application~1json/schema"),
        "{e}"
    );
    assert!(e.contains("external"), "{e}");
}

#[test]
fn json_specs_and_yaml_edge_cases() {
    let json = r#"{"openapi":"3.1.0","info":{"title":"x y","version":"1"},"paths":{}}"#;
    assert_eq!(parse_str(json).unwrap().name, "x");
    // `no` is a string, and an unquoted integer status code still works.
    let api = ok(
        "3.1.0",
        "components:
  schemas:
    Lang: {type: string, enum: [no, sv]}
paths:
  /a:
    get:
      responses:
        200: {description: ok}
        404: {description: missing}
",
    );
    let Shape::Enum { values } = &api.type_def("Lang").unwrap().shape else {
        panic!("Lang should be an enum");
    };
    assert_eq!(values, &["no", "sv"]);
    assert_eq!(api.resources[0].methods[0].errors[0].status, 404);
}

#[test]
fn rejects_swagger_2() {
    let e = parse_str("swagger: '2.0'\ninfo: {title: x, version: '1'}\n").unwrap_err();
    assert!(e.to_string().starts_with("#/openapi"), "{e}");
}

fn only_method(api: &Api) -> &oryva_ir::Method {
    &api.resources[0].methods[0]
}

#[test]
fn servers_are_named_from_extension_host_or_position() {
    let api = ok(
        "3.1.0",
        "servers:
  - {url: 'https://api.demo.example', description: Live, x-oryva-name: Production}
  - {url: 'https://sandbox.demo.example:8443/v1'}
  - {url: 'https://sandbox.other.example'}
  - {url: 'http://127.0.0.1:8080'}
  - url: 'https://{region}.demo.example'
    variables: {region: {default: eu}}
",
    );
    let got: Vec<(&str, &str)> = api
        .servers
        .iter()
        .map(|s| (s.name.as_str(), s.url.as_str()))
        .collect();
    assert_eq!(
        got,
        [
            ("production", "https://api.demo.example"),
            ("sandbox", "https://sandbox.demo.example:8443/v1"),
            ("environment_3", "https://sandbox.other.example"),
            ("environment_4", "http://127.0.0.1:8080"),
            ("eu", "https://eu.demo.example"),
        ]
    );
    assert_eq!(api.servers[0].description.as_deref(), Some("Live"));
    assert!(ok("3.0.3", "").servers.is_empty());
}

fn auth_spec(security: &str, schemes: &str) -> String {
    format!("security: {security}\ncomponents:\n  securitySchemes:\n{schemes}")
}

#[test]
fn auth_schemes_map_in_security_order() {
    let api = ok(
        "3.1.0",
        &auth_spec(
            "[{h: []}, {q: []}, {c: []}, {b: []}, {basic: []}, {oidc: []}, {implicit: [x]}]",
            "    unused: {type: http, scheme: bearer}
    h: {type: apiKey, in: header, name: x-api-key}
    q: {type: apiKey, in: query, name: key}
    c: {type: apiKey, in: cookie, name: sid}
    b: {type: http, scheme: Bearer}
    basic: {type: http, scheme: basic}
    oidc: {type: openIdConnect, openIdConnectUrl: 'https://x.example/.well-known'}
    implicit:
      type: oauth2
      flows: {implicit: {authorizationUrl: 'https://x.example/auth', scopes: {x: X}}}
",
        ),
    );
    let key = |location, name: &str| Auth::ApiKey {
        location,
        name: name.into(),
        env_var: "DEMO_API_KEY".into(),
    };
    let bearer = Auth::Bearer {
        env_var: "DEMO_API_KEY".into(),
    };
    assert_eq!(
        api.auth,
        [
            key(ApiKeyLocation::Header, "x-api-key"),
            key(ApiKeyLocation::Query, "key"),
            key(ApiKeyLocation::Cookie, "sid"),
            bearer.clone(),
            Auth::Basic {
                username_env: "DEMO_USERNAME".into(),
                password_env: "DEMO_PASSWORD".into()
            },
            bearer.clone(),
            bearer,
        ]
    );
}

#[test]
fn oauth2_client_credentials_scopes_come_from_requirement_else_flow() {
    let schemes = "    o:
      type: oauth2
      flows:
        clientCredentials:
          tokenUrl: 'https://auth.example/token'
          scopes: {a: A, b: B}
";
    let scopes = |security: &str| {
        let api = ok("3.0.3", &auth_spec(security, schemes));
        let [
            Auth::OAuth2ClientCredentials {
                token_url,
                scopes,
                client_id_env,
                client_secret_env,
            },
        ] = &api.auth[..]
        else {
            panic!("{:?}", api.auth);
        };
        assert_eq!(token_url, "https://auth.example/token");
        assert_eq!(client_id_env, "DEMO_CLIENT_ID");
        assert_eq!(client_secret_env, "DEMO_CLIENT_SECRET");
        scopes.clone()
    };
    assert_eq!(scopes("[{o: [b]}]"), ["b"]);
    assert_eq!(scopes("[{o: []}]"), ["a", "b"]);
}

#[test]
fn single_scheme_without_security_is_used_and_combined_requirements_are_an_error() {
    let api = ok(
        "3.0.3",
        "components:\n  securitySchemes:\n    k: {type: http, scheme: bearer}\n",
    );
    assert_eq!(api.auth.len(), 1);
    let both = "    a: {type: http, scheme: bearer}\n    b: {type: http, scheme: basic}\n";
    let e = err("3.0.3", &auth_spec("[{a: []}, {a: [], b: []}]", both));
    assert!(e.starts_with("#/security/1"), "{e}");
    assert!(e.contains("combine"), "{e}");
    let e = err("3.0.3", &auth_spec("[{missing: []}]", both));
    assert!(e.contains("securitySchemes/missing"), "{e}");
    let e = err(
        "3.0.3",
        &auth_spec("[{m: []}]", "    m: {type: mutualTLS}\n"),
    );
    assert!(e.contains("unsupported"), "{e}");
}

#[test]
fn idempotency_header_and_webhook_signing() {
    let api = ok("3.1.0", "");
    assert_eq!(api.idempotency_header, None);
    assert_eq!(api.webhook_signing, None);
    let api = parse_str(
        "openapi: 3.1.0
info:
  title: Demo API
  version: '1'
  x-oryva-idempotency-header: Idempotency-Key
  x-oryva-webhook-signing: standard-webhooks
",
    )
    .unwrap();
    assert_eq!(api.idempotency_header.as_deref(), Some("Idempotency-Key"));
    assert_eq!(
        api.webhook_signing,
        Some(WebhookSigning::StandardWebhooks {
            secret_env: "DEMO_WEBHOOK_SECRET".into()
        })
    );
    let e = parse_str(
        "openapi: 3.1.0\ninfo: {title: Demo, version: '1', x-oryva-webhook-signing: hmac}\n",
    )
    .unwrap_err()
    .to_string();
    assert!(e.starts_with("#/info/x-oryva-webhook-signing"), "{e}");
}

#[test]
fn webhooks_use_the_json_body_as_payload() {
    let api = ok(
        "3.1.0",
        "components:
  schemas:
    Paid: {type: object, properties: {id: {type: string}}}
webhooks:
  invoice.paid:
    post:
      summary: Paid.
      requestBody:
        content:
          application/json: {schema: {$ref: '#/components/schemas/Paid'}}
  payment.refund-failed:
    post:
      requestBody:
        content:
          application/json:
            schema: {type: object, properties: {reason: {type: string}}}
",
    );
    assert_eq!(api.webhooks.len(), 2);
    let (a, b) = (&api.webhooks[0], &api.webhooks[1]);
    assert_eq!(
        (a.name.as_str(), a.ident.as_str()),
        ("invoice.paid", "invoice_paid")
    );
    assert_eq!(a.description.as_deref(), Some("Paid."));
    assert_eq!(
        a.payload,
        TypeRef::Named {
            name: "Paid".into()
        }
    );
    assert_eq!(b.ident, "payment_refund_failed");
    assert_eq!(
        b.payload,
        TypeRef::Named {
            name: "PaymentRefundFailedWebhook".into()
        }
    );
    assert!(api.type_def("PaymentRefundFailedWebhook").is_some());

    let e = err(
        "3.1.0",
        "webhooks:\n  x.y:\n    post:\n      requestBody:\n        content:\n          text/plain: {schema: {type: string}}\n",
    );
    assert!(e.contains("#/webhooks/x.y/post/requestBody"), "{e}");
}

const PAGED_RESPONSE: &str = "responses:
        '200':
          description: ok
          content:
            application/json:
              schema:
                type: object
                properties:
                  data: {type: array, items: {type: string}}
                  meta:
                    type: object
                    properties: {next: {type: [string, 'null']}}
                  count: {type: integer}";

fn paged(pagination: &str) -> String {
    format!(
        "paths:
  /things:
    get:
      x-oryva-pagination: {pagination}
      parameters:
        - {{name: after, in: query, schema: {{type: string}}}}
        - {{name: offset, in: query, schema: {{type: integer}}}}
        - {{name: limit, in: query, schema: {{type: integer}}}}
        - {{name: page, in: query, schema: {{type: integer}}}}
        - {{name: in_header, in: header, schema: {{type: string}}}}
      {PAGED_RESPONSE}
"
    )
}

fn strings(p: &[&str]) -> Vec<String> {
    p.iter().map(|s| s.to_string()).collect()
}

#[test]
fn pagination_styles() {
    let get = |p: &str| only_method(&ok("3.1.0", &paged(p))).pagination.clone();
    assert_eq!(
        get("{style: cursor, param: after, next_cursor: [meta, next], items: [data]}"),
        Some(Pagination::Cursor {
            param: "after".into(),
            next_cursor: strings(&["meta", "next"]),
            items: strings(&["data"]),
        })
    );
    assert_eq!(
        get("{style: offset, offset_param: offset, limit_param: limit, items: [data]}"),
        Some(Pagination::Offset {
            offset_param: "offset".into(),
            limit_param: "limit".into(),
            items: strings(&["data"]),
        })
    );
    assert_eq!(
        get("{style: page, page_param: page, items: [data]}"),
        Some(Pagination::Page {
            page_param: "page".into(),
            items: strings(&["data"]),
        })
    );
}

#[test]
fn pagination_errors_carry_json_pointers() {
    let at = "#/paths/~1things/get/x-oryva-pagination";
    let cases = [
        ("{style: keyset}", "/style", "unknown style"),
        ("{param: after}", "/style", "required"),
        (
            "{style: cursor, next_cursor: [meta, next], items: [data]}",
            "/param",
            "required",
        ),
        (
            "{style: cursor, param: nope, next_cursor: [meta, next], items: [data]}",
            "/param",
            "no query parameter",
        ),
        (
            "{style: cursor, param: in_header, next_cursor: [meta, next], items: [data]}",
            "/param",
            "no query parameter",
        ),
        (
            "{style: cursor, param: after, next_cursor: [meta, gone], items: [data]}",
            "/next_cursor",
            "does not resolve",
        ),
        (
            "{style: cursor, param: after, items: [data]}",
            "/next_cursor",
            "required",
        ),
        (
            "{style: page, page_param: page, items: [count]}",
            "/items",
            "not an array",
        ),
        (
            "{style: page, page_param: page, items: [missing]}",
            "/items",
            "does not resolve",
        ),
        (
            "{style: offset, offset_param: offset, items: [data]}",
            "/limit_param",
            "required",
        ),
    ];
    for (ext, pointer, words) in cases {
        let e = err("3.1.0", &paged(ext));
        assert!(e.starts_with(&format!("{at}{pointer}:")), "{ext}: {e}");
        assert!(e.contains(words), "{ext}: {e}");
    }
}

#[test]
fn event_stream_response_takes_event_from_schema() {
    let api = ok(
        "3.1.0",
        "components:
  schemas:
    Tick: {type: object, properties: {n: {type: integer}}}
paths:
  /ticks:
    get:
      responses:
        '200':
          description: ok
          content:
            text/event-stream: {schema: {$ref: '#/components/schemas/Tick'}}
  /inline:
    get:
      responses:
        '200':
          description: ok
          content:
            'text/event-stream; charset=utf-8':
              schema: {type: object, properties: {m: {type: string}}}
",
    );
    let resources = &api.resources;
    assert_eq!(
        resources[0].methods[0].response,
        ResponseBody::EventStream {
            event: TypeRef::Named {
                name: "Tick".into()
            }
        }
    );
    assert_eq!(
        resources[1].methods[0].response,
        ResponseBody::EventStream {
            event: TypeRef::Named {
                name: "InlineListEvent".into()
            }
        }
    );
}

#[test]
fn binary_request_bodies() {
    let body = |content: &str| {
        let api = ok(
            "3.1.0",
            &format!(
                "paths:\n  /up:\n    post:\n      requestBody:\n        required: true\n        content:\n{content}\n      {R200}\n"
            ),
        );
        only_method(&api).body.clone().unwrap()
    };
    let b = body("          application/octet-stream: {schema: {type: string, format: binary}}");
    assert_eq!((b.encoding, b.ty), (BodyEncoding::Binary, TypeRef::File));
    assert!(b.required);
    let b = body("          application/octet-stream: {}");
    assert_eq!(b.encoding, BodyEncoding::Binary);
    let b = body("          image/png: {schema: {type: string, contentMediaType: image/png}}");
    assert_eq!((b.encoding, b.ty), (BodyEncoding::Binary, TypeRef::File));
    // A body with no content type at all is still an error.
    let e = err(
        "3.1.0",
        &format!(
            "paths:\n  /up:\n    post:\n      requestBody:\n        content: {{}}\n      {R200}\n"
        ),
    );
    assert!(e.contains("requestBody"), "{e}");
}

#[test]
fn defaults_on_params_and_fields_including_beside_all_of() {
    let api = ok(
        "3.0.3",
        "components:
  schemas:
    Cur: {type: string, enum: [usd, eur]}
    Body:
      type: object
      properties:
        currency:
          allOf: [{$ref: '#/components/schemas/Cur'}]
          default: usd
        qty: {type: integer, default: 1}
        none: {type: string}
paths:
  /a:
    get:
      parameters:
        - {name: limit, in: query, schema: {type: integer, default: 20}}
        - {name: q, in: query, schema: {type: string}}
      responses: {'200': {description: ok}}
",
    );
    let m = only_method(&api);
    assert_eq!(m.params[0].default, Some(20.into()));
    assert_eq!(m.params[1].default, None);
    let Shape::Object { fields } = &api.type_def("Body").unwrap().shape else {
        panic!()
    };
    let got: Vec<_> = fields.iter().map(|f| f.default.clone()).collect();
    assert_eq!(got, [Some("usd".into()), Some(1.into()), None]);
    assert_eq!(fields[0].ty, TypeRef::Named { name: "Cur".into() });
}

#[test]
fn integer_enums_keep_numbers_and_odd_enums_fall_back() {
    let api = ok(
        "3.1.0",
        "components:
  schemas:
    A:
      type: object
      properties:
        level: {type: integer, enum: [0, 1, 2]}
        mixed: {enum: [a, 1]}
        floats: {type: number, enum: [1.5, 2.5]}
        flag: {type: boolean, enum: [true]}
    Level: {type: integer, enum: [1, 2]}
",
    );
    assert_eq!(
        field_ty(&api, "A", "level"),
        TypeRef::Enum {
            values: vec![0.into(), 1.into(), 2.into()]
        }
    );
    assert_eq!(field_ty(&api, "A", "mixed"), TypeRef::Any);
    assert_eq!(field_ty(&api, "A", "floats"), TypeRef::Number);
    assert_eq!(field_ty(&api, "A", "flag"), TypeRef::Boolean);
    assert_eq!(
        api.type_def("Level").unwrap().shape,
        Shape::Enum {
            values: vec![1.into(), 2.into()]
        }
    );
}

fn union_of(api: &Api, name: &str) -> Vec<(String, String)> {
    let Shape::Union {
        discriminator: Some(d),
        ..
    } = &api.type_def(name).unwrap().shape
    else {
        panic!("{name} is not a discriminated union");
    };
    d.mapping
        .iter()
        .map(|c| (c.value.clone(), c.type_name.clone()))
        .collect()
}

#[test]
fn discriminator_mapping_from_explicit_const_enum_or_name() {
    let api = ok(
        "3.1.0",
        "components:
  schemas:
    Cat: {type: object, properties: {kind: {type: string, const: kitty}}}
    Dog: {type: object, properties: {kind: {type: string, enum: [doggo]}}}
    Fish: {type: object, properties: {kind: {type: string}}}
    Pet:
      oneOf:
        - $ref: '#/components/schemas/Cat'
        - $ref: '#/components/schemas/Dog'
        - $ref: '#/components/schemas/Fish'
      discriminator: {propertyName: kind}
    Mapped:
      anyOf:
        - $ref: '#/components/schemas/Cat'
        - $ref: '#/components/schemas/Dog'
      discriminator:
        propertyName: kind
        mapping: {feline: '#/components/schemas/Cat', Dog: Dog}
",
    );
    assert_eq!(
        union_of(&api, "Pet"),
        [
            ("kitty".into(), "Cat".into()),
            ("doggo".into(), "Dog".into()),
            ("Fish".into(), "Fish".into())
        ]
    );
    assert_eq!(
        union_of(&api, "Mapped"),
        [
            ("feline".into(), "Cat".into()),
            ("Dog".into(), "Dog".into())
        ]
    );
}

#[test]
fn nullable_component_references_are_wrapped() {
    let api = ok(
        "3.0.3",
        "components:
  schemas:
    Maybe: {type: object, nullable: true, properties: {x: {type: string}}}
    Tag: {type: string, enum: [a, b], nullable: true}
    Holder:
      type: object
      properties:
        a: {$ref: '#/components/schemas/Maybe'}
        b: {allOf: [{$ref: '#/components/schemas/Maybe'}]}
        c: {type: array, items: {$ref: '#/components/schemas/Tag'}}
",
    );
    let maybe = nullable(TypeRef::Named {
        name: "Maybe".into(),
    });
    assert_eq!(field_ty(&api, "Holder", "a"), maybe);
    assert_eq!(field_ty(&api, "Holder", "b"), maybe);
    assert_eq!(
        field_ty(&api, "Holder", "c"),
        TypeRef::Array {
            items: Box::new(nullable(TypeRef::Named { name: "Tag".into() }))
        }
    );
    // The definition itself is not nullable; only references are.
    assert!(matches!(
        api.type_def("Maybe").unwrap().shape,
        Shape::Object { .. }
    ));
}

fn edge_cases() -> Api {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/specs/edge-cases.yaml");
    parse_file(&path).unwrap()
}

fn edge_items(api: &Api) -> &[oryva_ir::Method] {
    &api.resources
        .iter()
        .find(|r| r.name == "items")
        .unwrap()
        .methods
}

#[test]
fn hoisted_object_that_shares_a_component_name_gets_a_number() {
    let api = edge_cases();
    let create = edge_items(&api)
        .iter()
        .find(|m| m.name == "create")
        .unwrap();
    // The component `ItemsCreateRequest` keeps its name; the inline body becomes `...2`.
    assert_eq!(
        create.body.as_ref().unwrap().ty,
        TypeRef::Named {
            name: "ItemsCreateRequest2".into()
        }
    );
    assert!(api.type_def("ItemsCreateRequest").is_some());
}

#[test]
fn any_other_content_type_is_a_raw_body_that_keeps_its_type() {
    let api = edge_cases();
    let body = |name: &str| {
        let m = edge_items(&api).iter().find(|m| m.name == name).unwrap();
        m.body.clone().unwrap()
    };
    let avatar = body("set_avatar");
    assert_eq!(avatar.encoding, BodyEncoding::Binary);
    assert_eq!(avatar.content_type.as_deref(), Some("image/jpeg"));
    assert_eq!(body("add_note").content_type.as_deref(), Some("text/plain"));
    // JSON keeps the encoding's standard type.
    assert_eq!(body("create").content_type, None);
    // A wildcard range is not something to send.
    let api = ok(
        "3.0.3",
        &format!(
            "paths:\n  /x:\n    put:\n      requestBody: {{content: {{'*/*': {{}}}}}}\n      {R200}\n"
        ),
    );
    let body = api.resources[0].methods[0].body.clone().unwrap();
    assert_eq!(
        body.content_type.as_deref(),
        Some("application/octet-stream")
    );
}

#[test]
fn spec_larger_than_the_yaml_node_budget_parses() {
    // The YAML parser's default budget stops at 250,000 nodes; real specs are bigger.
    let values: String = (0..300_000).map(|i| format!("a{i},")).collect();
    let api = ok(
        "3.0.3",
        &format!("components:\n  schemas:\n    Big: {{type: string, enum: [{values}]}}\n"),
    );
    assert_eq!(api.types.len(), 1);
}
