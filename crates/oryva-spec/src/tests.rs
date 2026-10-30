use super::*;
use oryva_ir::{Shape, TypeRef};

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
