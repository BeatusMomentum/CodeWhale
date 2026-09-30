//! The TypeScript half of the protocol, generated from the Rust types.
//!
//! `schemars` reads each wire type's serde shape (field names, `Option` and
//! `default` fields, `deny_unknown_fields`, tags); [`render`] normalizes that
//! into the small vocabulary the host's validator checks and renders
//! `extension-host/src/protocol.generated.ts`. The committed file must equal
//! the rendering: re-record it with `CODEWHALE_CONFORMANCE_UPDATE=1` (refused
//! under CI, like every conformance golden), then rebuild `dist/`.
//!
//! Known limit: a wire shape outside that vocabulary (floats, inline
//! objects, unions in params) panics here rather than being approximated.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::Path;

use schemars::generate::SchemaSettings;
use schemars::{Schema, SchemaGenerator};

// `Value` and every wire type come from the parent module.
use super::*;

/// The params type of every method in [`METHODS`].
fn params_schema(method: &str, generator: &mut SchemaGenerator) -> Schema {
    match method {
        "host/initialize" => generator.subschema_for::<InitializeParams>(),
        "host/ping" | "host/shutdown" | "host/ready" => generator.subschema_for::<EmptyParams>(),
        "ext/activate" => generator.subschema_for::<ActivateParams>(),
        "ext/deactivate" => generator.subschema_for::<DeactivateParams>(),
        "tool/call" => generator.subschema_for::<ToolCallParams>(),
        "$/cancel" => generator.subschema_for::<CancelParams>(),
        "host/hello" => generator.subschema_for::<HelloParams>(),
        "registry/register" => generator.subschema_for::<RegisterParams>(),
        "registry/unregister" => generator.subschema_for::<UnregisterParams>(),
        "ext/faulted" => generator.subschema_for::<FaultedParams>(),
        "log" => generator.subschema_for::<LogParams>(),
        other => panic!("method `{other}` has no params type in the TypeScript generator"),
    }
}

/// A field's wire type, normalized.
enum Ty {
    String,
    Boolean,
    Uint,
    Integer,
    /// A JSON object with any keys (`Map<String, Value>`).
    Object,
    /// Any JSON value.
    Json,
    Ref(String),
    /// A string tag inside a union member.
    Const(String),
    Array(Box<Ty>),
}

struct Field {
    name: String,
    ty: Ty,
    required: bool,
}

enum Def {
    Object { strict: bool, fields: Vec<Field> },
    Enum(Vec<String>),
    Union(Vec<Vec<Field>>),
}

fn ref_name(schema: &Value) -> Option<String> {
    let reference = schema.get("$ref")?.as_str()?;
    Some(
        reference
            .strip_prefix("#/$defs/")
            .unwrap_or_else(|| panic!("unexpected $ref `{reference}`"))
            .to_string(),
    )
}

fn parse_ty(at: &str, schema: &Value) -> Ty {
    let Some(object) = schema.as_object() else {
        assert_eq!(schema, &Value::Bool(true), "{at}: unsupported schema");
        return Ty::Json;
    };
    if let Some(name) = ref_name(schema) {
        return Ty::Ref(name);
    }
    if let Some(value) = object.get("const").and_then(Value::as_str) {
        return Ty::Const(value.to_string());
    }
    let types: Vec<&str> = match object.get("type") {
        Some(Value::String(ty)) => vec![ty.as_str()],
        Some(Value::Array(types)) => types
            .iter()
            .filter_map(Value::as_str)
            .filter(|ty| *ty != "null")
            .collect(),
        // Only annotations (`default`, `description`): any value.
        None if !["enum", "anyOf", "oneOf", "allOf", "properties", "items"]
            .iter()
            .any(|key| object.contains_key(*key)) =>
        {
            return Ty::Json;
        }
        _ => panic!("{at}: unsupported wire schema {schema}"),
    };
    match types.as_slice() {
        ["string"] => Ty::String,
        ["boolean"] => Ty::Boolean,
        ["integer"] if object.get("minimum").and_then(Value::as_u64) == Some(0) => Ty::Uint,
        ["integer"] => Ty::Integer,
        ["object"]
            if object.get("additionalProperties") == Some(&Value::Bool(true))
                && !object.contains_key("properties") =>
        {
            Ty::Object
        }
        ["array"] => Ty::Array(Box::new(parse_ty(
            &format!("{at}[]"),
            object
                .get("items")
                .unwrap_or_else(|| panic!("{at}: array without items")),
        ))),
        _ => panic!("{at}: unsupported wire schema {schema}"),
    }
}

fn object_fields(at: &str, schema: &Value) -> Vec<Field> {
    assert_eq!(
        schema.get("type").and_then(Value::as_str),
        Some("object"),
        "{at}: expected an object schema, got {schema}"
    );
    let required: Vec<&str> = schema
        .get("required")
        .and_then(Value::as_array)
        .map(|names| names.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    schema
        .get("properties")
        .and_then(Value::as_object)
        .map(|properties| {
            properties
                .iter()
                .map(|(name, schema)| Field {
                    name: name.clone(),
                    ty: parse_ty(&format!("{at}.{name}"), schema),
                    required: required.contains(&name.as_str()),
                })
                .collect()
        })
        .unwrap_or_default()
}

fn parse_def(name: &str, schema: &Value) -> Def {
    let members = schema
        .get("oneOf")
        .or_else(|| schema.get("anyOf"))
        .and_then(Value::as_array);
    if let Some(members) = members {
        // Unit variants, with or without doc comments: a string enum.
        let consts: Option<Vec<String>> = members
            .iter()
            .map(|member| {
                member
                    .get("const")
                    .and_then(Value::as_str)
                    .map(str::to_string)
            })
            .collect();
        return match consts {
            Some(values) => Def::Enum(values),
            None => Def::Union(
                members
                    .iter()
                    .map(|member| object_fields(name, member))
                    .collect(),
            ),
        };
    }
    if let Some(values) = schema.get("enum").and_then(Value::as_array) {
        return Def::Enum(
            values
                .iter()
                .map(|value| {
                    value
                        .as_str()
                        .unwrap_or_else(|| panic!("{name}: non-string enum value"))
                        .to_string()
                })
                .collect(),
        );
    }
    Def::Object {
        strict: schema.get("additionalProperties") == Some(&Value::Bool(false)),
        fields: object_fields(name, schema),
    }
}

fn quote(value: &str) -> String {
    assert!(
        !value.contains(['\'', '\\']),
        "`{value}` needs escaping in TypeScript"
    );
    format!("'{value}'")
}

/// The validator's kind for `ty`: a string enum is inlined, an object is a
/// reference into `SHAPES`.
fn kind(ty: &Ty, defs: &BTreeMap<String, Def>) -> String {
    match ty {
        Ty::String => "'string'".into(),
        Ty::Boolean => "'boolean'".into(),
        Ty::Uint => "'uint'".into(),
        Ty::Integer => "'integer'".into(),
        Ty::Object => "'object'".into(),
        Ty::Json => "'json'".into(),
        Ty::Ref(name) => match &defs[name] {
            Def::Object { .. } => format!("{{ ref: {} }}", quote(name)),
            Def::Enum(values) => format!(
                "{{ enum: [{}] }}",
                values
                    .iter()
                    .map(|v| quote(v))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            Def::Union(_) => panic!("{name}: the host validator does not check unions"),
        },
        Ty::Const(value) => panic!("`{value}`: a tag outside a union"),
        Ty::Array(item) => format!("{{ items: {} }}", kind(item, defs)),
    }
}

fn kinds<'a>(fields: impl Iterator<Item = &'a Field>, defs: &BTreeMap<String, Def>) -> String {
    let rendered: Vec<String> = fields
        .map(|field| format!("{}: {}", field.name, kind(&field.ty, defs)))
        .collect();
    if rendered.is_empty() {
        "{}".into()
    } else {
        format!("{{ {} }}", rendered.join(", "))
    }
}

fn ts(ty: &Ty) -> String {
    match ty {
        Ty::String => "string".into(),
        Ty::Boolean => "boolean".into(),
        Ty::Uint | Ty::Integer => "number".into(),
        Ty::Object => "{ [key: string]: Json }".into(),
        Ty::Json => "Json".into(),
        Ty::Ref(name) => name.clone(),
        Ty::Const(value) => quote(value),
        Ty::Array(item) => format!("{}[]", ts(item)),
    }
}

fn member(field: &Field) -> String {
    let optional = if field.required { "" } else { "?" };
    format!("{}{optional}: {}", field.name, ts(&field.ty))
}

const HEADER: &str = "\
// @generated from the Rust protocol types in crates/tui/src/extension_host/protocol.rs
// by `extension_host::protocol::tests`. Do not edit: change the Rust side, re-record with
//   CODEWHALE_CONFORMANCE_UPDATE=1 cargo test -p codewhale-tui --lib extension_host::protocol
// and rebuild dist/ with `npm run build`.

";

const VALIDATOR_TYPES: &str = "
/** A field's wire kind: the Rust field's serde type, normalized for validation. */
export type Kind =
  | 'string'
  | 'boolean'
  | 'uint'
  | 'integer'
  | 'object'
  | 'json'
  | { readonly ref: string }
  | { readonly enum: readonly string[] }
  | { readonly items: Kind }

/** An object's fields; `strict` is Rust's `deny_unknown_fields`. */
export interface Shape {
  readonly strict: boolean
  readonly required: { readonly [field: string]: Kind }
  readonly optional: { readonly [field: string]: Kind }
}
";

/// Render `protocol.generated.ts` from [`METHODS`] and the wire types.
fn render() -> String {
    let mut generator = SchemaSettings::draft2020_12().into_generator();
    let methods: Vec<(&MethodSpec, String)> = METHODS
        .iter()
        .map(|spec| {
            let schema = params_schema(spec.name, &mut generator);
            let name = ref_name(schema.as_value())
                .unwrap_or_else(|| panic!("{}: params must be a named type", spec.name));
            (spec, name)
        })
        .collect();
    let error = generator.subschema_for::<RpcErrorWire>();
    let error = ref_name(error.as_value()).expect("RpcErrorWire is a named type");
    // Results are typed for the host, not validated by it.
    let _ = generator.subschema_for::<RegisterResult>();
    let _ = generator.subschema_for::<ActivateResult>();
    let _ = generator.subschema_for::<DeactivateResult>();
    let _ = generator.subschema_for::<ToolResultWire>();
    let defs: BTreeMap<String, Def> = generator
        .definitions()
        .iter()
        .map(|(name, schema)| (name.clone(), parse_def(name, schema)))
        .collect();

    // What the validator checks: every params type and the error object,
    // with the objects they reference.
    let mut validated = BTreeSet::new();
    let mut pending: Vec<String> = methods
        .iter()
        .map(|(_, name)| name.clone())
        .chain([error])
        .collect();
    while let Some(name) = pending.pop() {
        if let Def::Object { fields, .. } = &defs[&name]
            && validated.insert(name.clone())
        {
            for field in fields {
                let mut ty = &field.ty;
                while let Ty::Array(item) = ty {
                    ty = item;
                }
                if let Ty::Ref(name) = ty {
                    pending.push(name.clone());
                }
            }
        }
    }

    let mut out = String::from(HEADER);
    let magic = std::str::from_utf8(&MAGIC).expect("ASCII magic");
    let _ = writeln!(out, "export const PROTOCOL_VERSION = {PROTOCOL_VERSION}");
    let _ = writeln!(out, "export const MAGIC_ASCII = {}", quote(magic));
    let _ = writeln!(out, "export const HEADER_LEN = {HEADER_LEN}");
    let _ = writeln!(out, "export const MAX_FRAME = {MAX_FRAME}");
    let _ = writeln!(out, "export const MAX_INFLIGHT = {MAX_INFLIGHT}");
    out.push_str(
        "\n/** JSON-RPC error codes used on this channel. */\nexport const ErrorCode = {\n",
    );
    for (name, code) in error_code::ALL {
        let _ = writeln!(out, "  {name}: {code},");
    }
    out.push_str("} as const\n\nexport type Direction = 'core_to_host' | 'host_to_core'\n");
    out.push_str("\n/** Every method either side may send; nothing else is admitted. */\nexport const METHODS = [\n");
    for (spec, params) in &methods {
        let _ = writeln!(
            out,
            "  {{ name: {}, direction: {}, request: {}, params: {} }},",
            quote(spec.name),
            quote(spec.direction.as_str()),
            spec.request,
            quote(params)
        );
    }
    out.push_str("] as const\n");
    out.push_str(VALIDATOR_TYPES);
    out.push_str("\n/** Every method's params, and an error response's `error`. */\nexport const SHAPES: { readonly [name: string]: Shape } = {\n");
    for name in &validated {
        let Def::Object { strict, fields } = &defs[name] else {
            unreachable!("only objects are validated");
        };
        let _ = writeln!(out, "  {name}: {{");
        let _ = writeln!(out, "    strict: {strict},");
        let _ = writeln!(
            out,
            "    required: {},",
            kinds(fields.iter().filter(|field| field.required), &defs)
        );
        let _ = writeln!(
            out,
            "    optional: {},",
            kinds(fields.iter().filter(|field| !field.required), &defs)
        );
        out.push_str("  },\n");
    }
    out.push_str(
        "}\n\nexport type Json = null | boolean | number | string | Json[] | { [key: string]: Json }\n",
    );
    for (name, def) in &defs {
        out.push('\n');
        match def {
            Def::Object { fields, .. } if fields.is_empty() => {
                let _ = writeln!(out, "export interface {name} {{}}");
            }
            Def::Object { fields, .. } => {
                let _ = writeln!(out, "export interface {name} {{");
                for field in fields {
                    let _ = writeln!(out, "  {}", member(field));
                }
                out.push_str("}\n");
            }
            Def::Enum(values) => {
                let values: Vec<String> = values.iter().map(|v| quote(v)).collect();
                let _ = writeln!(out, "export type {name} = {}", values.join(" | "));
            }
            Def::Union(members) => {
                let members: Vec<String> = members
                    .iter()
                    .map(|fields| {
                        // Tags first, then the variant's fields in order.
                        let (tags, rest): (Vec<&Field>, Vec<&Field>) = fields
                            .iter()
                            .partition(|field| matches!(field.ty, Ty::Const(_)));
                        let parts: Vec<String> = tags.into_iter().chain(rest).map(member).collect();
                        format!("{{ {} }}", parts.join("; "))
                    })
                    .collect();
                let _ = writeln!(out, "export type {name} = {}", members.join(" | "));
            }
        }
    }
    out
}

#[test]
fn typescript_protocol_is_generated_from_the_rust_types() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("extension-host")
        .join("src")
        .join("protocol.generated.ts");
    if let Err(drift) = crate::conformance::golden::check_golden(&path, &render()) {
        panic!(
            "{drift}\nThe TypeScript protocol is generated from protocol.rs; rebuild dist/ after re-recording."
        );
    }
}
