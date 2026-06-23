//! Generated TypeScript codecs: per-type `encode`/`decode` functions that bridge
//! the in-memory representation and the JSON wire form for the cases plain
//! `JSON.stringify` cannot handle (i64 as string, bytes as base64, branded
//! well-known types). Plain JSON-native fields pass through untouched. Encoders
//! emit wire keys; decoders read wire keys and produce in-code identifiers, which
//! is where the identifier/wire-key split materializes.

use crate::codegen::casing::{self, CasingConfig};
use crate::codegen::symbol::{Symbol, SymbolKind};
use crate::codegen::targets::typescript::types::{field_ident, type_ident, wire_key};
use crate::codegen::tree::{Decl, Field, FnBody, Function, TypeExpr};
use crate::ir::{Member, Prim, Shape, ShapeKind, Tref};

/// The shared runtime helpers a generated file relies on, emitted once per file
/// with zero dependencies.
pub fn runtime_helpers() -> Vec<Decl> {
    vec![
        function(
            "encodeI64",
            &[("v", "bigint")],
            "string",
            "  return v.toString();",
        ),
        function(
            "decodeI64",
            &[("s", "string")],
            "bigint",
            "  return BigInt(s);",
        ),
        function(
            "encodeBytes",
            &[("b", "Uint8Array")],
            "string",
            "  return btoa(String.fromCharCode(...b));",
        ),
        function(
            "decodeBytes",
            &[("s", "string")],
            "Uint8Array",
            "  return Uint8Array.from(atob(s), (c) => c.charCodeAt(0));",
        ),
    ]
}

/// Emit the encode/decode codecs for a shape. Unions are handled by a later
/// phase.
pub fn emit_codecs(shape: &Shape, config: &CasingConfig) -> Vec<Decl> {
    match &shape.kind {
        ShapeKind::Structure { members, .. } => struct_codecs(shape, members, config),
        ShapeKind::Enum { .. } => enum_codecs(shape, config),
        _ => Vec::new(),
    }
}

fn struct_codecs(shape: &Shape, members: &[Member], config: &CasingConfig) -> Vec<Decl> {
    let ty = type_ident(shape, config);
    let encode_fields: String = members
        .iter()
        .map(|m| {
            let access = format!("value.{}", field_ident(m, config));
            let expr = guard_null(m, &access, encode_expr(&access, &m.target, config));
            format!("    {}: {expr},\n", wire_key(m))
        })
        .collect();
    let decode_fields: String = members
        .iter()
        .map(|m| {
            let access = format!("raw.{}", wire_key(m));
            let expr = guard_null(m, &access, decode_expr(&access, &m.target, config));
            format!("    {}: {expr},\n", field_ident(m, config))
        })
        .collect();
    vec![
        function_owned(
            &format!("encode{ty}"),
            &[("value", &ty)],
            "unknown",
            format!("  return {{\n{encode_fields}  }};"),
        ),
        function_owned(
            &format!("decode{ty}"),
            &[("raw", "any")],
            &ty,
            format!("  return {{\n{decode_fields}  }};"),
        ),
    ]
}

fn enum_codecs(shape: &Shape, config: &CasingConfig) -> Vec<Decl> {
    // An open enum is a string on the wire; encode is identity and decode is a
    // lenient cast that lets an unknown value pass through.
    let ty = type_ident(shape, config);
    vec![
        function_owned(
            &format!("encode{ty}"),
            &[("value", &ty)],
            "string",
            "  return value;".into(),
        ),
        function_owned(
            &format!("decode{ty}"),
            &[("raw", "string")],
            &ty,
            format!("  return raw as {ty};"),
        ),
    ]
}

/// Wrap an optional member's conversion so an absent value is omitted.
fn guard_null(member: &Member, access: &str, expr: String) -> String {
    if member.required {
        expr
    } else {
        format!("{access} == null ? undefined : {expr}")
    }
}

/// The TypeScript expression that encodes `value` of IR type `t` to its wire
/// form. Only the cases that differ from JSON-native pass through a codec.
fn encode_expr(value: &str, t: &Tref, config: &CasingConfig) -> String {
    match t {
        Tref::Prim(Prim::I64 | Prim::U64) => format!("encodeI64({value})"),
        Tref::Prim(Prim::Bytes) => format!("encodeBytes({value})"),
        // Native types and branded strings are already wire-shaped.
        Tref::Prim(_) | Tref::Param(_) => value.to_string(),
        Tref::Ref { id, .. } => format!("encode{}({value})", pascal_local(id, config)),
        Tref::List(inner) => format!("{value}.map((x) => {})", encode_expr("x", inner, config)),
        Tref::Map(_, v) => format!(
            "Object.fromEntries(Object.entries({value}).map(([k, v]) => [k, {}]))",
            encode_expr("v", v, config)
        ),
    }
}

/// The TypeScript expression that decodes `value` of IR type `t` from its wire
/// form into the in-memory representation.
fn decode_expr(value: &str, t: &Tref, config: &CasingConfig) -> String {
    match t {
        Tref::Prim(Prim::I64 | Prim::U64) => format!("decodeI64({value})"),
        Tref::Prim(Prim::Bytes) => format!("decodeBytes({value})"),
        Tref::Prim(Prim::Timestamp) => format!("({value} as Timestamp)"),
        Tref::Prim(Prim::Date) => format!("({value} as LocalDate)"),
        Tref::Prim(Prim::Duration) => format!("({value} as Duration)"),
        Tref::Prim(Prim::Uuid) => format!("({value} as Uuid)"),
        Tref::Prim(_) | Tref::Param(_) => value.to_string(),
        Tref::Ref { id, .. } => format!("decode{}({value})", pascal_local(id, config)),
        Tref::List(inner) => {
            format!(
                "{value}.map((x: any) => {})",
                decode_expr("x", inner, config)
            )
        }
        Tref::Map(_, v) => format!(
            "Object.fromEntries(Object.entries({value}).map(([k, v]: [string, any]) => [k, {}]))",
            decode_expr("v", v, config)
        ),
    }
}

/// The PascalCase codec suffix for a referenced type (its name after `module#`).
/// A referenced type's own `@rename` is not visible here, so casing applies.
fn pascal_local(id: &str, config: &CasingConfig) -> String {
    let local = id.rsplit('#').next().unwrap_or(id);
    casing::transform(local, SymbolKind::Type, config, None)
}

fn function(name: &str, params: &[(&str, &str)], ret: &str, body: &str) -> Decl {
    function_owned(name, params, ret, body.to_string())
}

fn function_owned(name: &str, params: &[(&str, &str)], ret: &str, body: String) -> Decl {
    Decl::Function(Function {
        name: Symbol::builtin(name),
        params: params
            .iter()
            .map(|&(n, t)| Field {
                name: Symbol::builtin(n),
                ty: TypeExpr::Ref(Symbol::builtin(t)),
                nullable: false,
                wire: None,
            })
            .collect(),
        ret: Some(TypeExpr::Ref(Symbol::builtin(ret))),
        body: FnBody::Raw {
            text: body,
            refs: Vec::new(),
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codegen::target::RenderRules;
    use crate::codegen::targets::typescript::types::ts_casing;
    use crate::codegen::targets::typescript::TsRules;

    fn member(name: &str, target: Tref, required: bool) -> Member {
        Member {
            name: name.into(),
            target,
            required,
            default: None,
            constraints: vec![],
            traits: vec![],
        }
    }

    fn rendered(decls: &[Decl]) -> String {
        decls
            .iter()
            .map(|d| TsRules.render_decl(d))
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn runtime_helpers_cover_i64_and_bytes() {
        let out = rendered(&runtime_helpers());
        assert!(out.contains("export function encodeI64(v: bigint): string {"));
        assert!(out.contains("return v.toString();"));
        assert!(out.contains("export function decodeI64(s: string): bigint {"));
        assert!(out.contains("return BigInt(s);"));
        assert!(out.contains("export function encodeBytes(b: Uint8Array): string {"));
        assert!(out.contains("export function decodeBytes(s: string): Uint8Array {"));
    }

    #[test]
    fn struct_codec_routes_i64_and_uses_wire_keys() {
        let shape = Shape {
            id: "billing#charge".into(),
            kind: ShapeKind::Structure {
                params: vec![],
                members: vec![
                    member("amount_cents", Tref::Prim(Prim::I64), true),
                    member("note", Tref::Prim(Prim::String), false),
                ],
            },
            traits: vec![],
        };
        let out = rendered(&emit_codecs(&shape, &ts_casing()));
        assert!(out.contains("export function encodeCharge(value: Charge): unknown {"));
        // encode: wire key out, in-code identifier read, i64 routed.
        assert!(out.contains("amount_cents: encodeI64(value.amountCents),"));
        // optional field omitted when null.
        assert!(out.contains("note: value.note == null ? undefined : value.note,"));
        assert!(out.contains("export function decodeCharge(raw: any): Charge {"));
        assert!(out.contains("amountCents: decodeI64(raw.amount_cents),"));
    }

    #[test]
    fn open_enum_codec_is_identity_and_lenient() {
        let shape = Shape {
            id: "billing#status".into(),
            kind: ShapeKind::Enum {
                backing: crate::ir::EnumBacking::String,
                values: vec![("pending".into(), None)],
            },
            traits: vec![],
        };
        let out = rendered(&emit_codecs(&shape, &ts_casing()));
        assert!(out.contains("export function encodeStatus(value: Status): string {"));
        assert!(out.contains("return value;"));
        assert!(out.contains("export function decodeStatus(raw: string): Status {"));
        assert!(out.contains("return raw as Status;"));
    }

    #[test]
    fn unsupported_shapes_emit_no_codecs() {
        let service = Shape {
            id: "billing#Api".into(),
            kind: ShapeKind::Service { operations: vec![] },
            traits: vec![],
        };
        assert!(emit_codecs(&service, &ts_casing()).is_empty());
    }

    #[test]
    fn expressions_compose_over_collections_refs_and_branded() {
        let cfg = ts_casing();
        // bytes, branded, ref, list, map, param, native.
        assert_eq!(
            encode_expr("x", &Tref::Prim(Prim::Bytes), &cfg),
            "encodeBytes(x)"
        );
        assert_eq!(encode_expr("x", &Tref::Prim(Prim::Bool), &cfg), "x");
        assert_eq!(encode_expr("x", &Tref::Param("T".into()), &cfg), "x");
        assert_eq!(
            decode_expr("x", &Tref::Prim(Prim::Timestamp), &cfg),
            "(x as Timestamp)"
        );
        assert_eq!(
            decode_expr("x", &Tref::Prim(Prim::Date), &cfg),
            "(x as LocalDate)"
        );
        assert_eq!(
            decode_expr("x", &Tref::Prim(Prim::Duration), &cfg),
            "(x as Duration)"
        );
        assert_eq!(
            decode_expr("x", &Tref::Prim(Prim::Uuid), &cfg),
            "(x as Uuid)"
        );
        assert_eq!(
            encode_expr(
                "x",
                &Tref::Ref {
                    id: "m#Charge".into(),
                    args: vec![]
                },
                &cfg
            ),
            "encodeCharge(x)"
        );
        assert_eq!(
            decode_expr(
                "x",
                &Tref::Ref {
                    id: "m#Charge".into(),
                    args: vec![]
                },
                &cfg
            ),
            "decodeCharge(x)"
        );
        assert_eq!(
            encode_expr("xs", &Tref::List(Box::new(Tref::Prim(Prim::I64))), &cfg),
            "xs.map((x) => encodeI64(x))"
        );
        assert_eq!(
            decode_expr("xs", &Tref::List(Box::new(Tref::Prim(Prim::I64))), &cfg),
            "xs.map((x: any) => decodeI64(x))"
        );
        assert!(encode_expr(
            "m",
            &Tref::Map(
                Box::new(Tref::Prim(Prim::String)),
                Box::new(Tref::Prim(Prim::Bytes))
            ),
            &cfg
        )
        .contains("encodeBytes(v)"));
        assert!(decode_expr(
            "m",
            &Tref::Map(
                Box::new(Tref::Prim(Prim::String)),
                Box::new(Tref::Prim(Prim::Bytes))
            ),
            &cfg
        )
        .contains("decodeBytes(v)"));
    }
}
