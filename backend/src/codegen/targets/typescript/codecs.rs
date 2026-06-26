//! Generated TypeScript codecs: per-type `encode`/`decode` functions that bridge
//! the in-memory representation and the JSON wire form for the cases plain
//! `JSON.stringify` cannot handle (i64 as string, bytes as base64, branded
//! well-known types). Plain JSON-native fields pass through untouched. Encoders
//! emit wire keys; decoders read wire keys and produce in-code identifiers, which
//! is where the identifier/wire-key split materializes.

use crate::codegen::casing::CasingConfig;
use crate::codegen::conventions::{field_ident, has_entries, type_ident, wire_key};
use crate::codegen::symbol::Symbol;
use crate::codegen::targets::typescript::types::LANG;
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

/// Emit the encode/decode codecs for a shape. The codecs live in the serde file,
/// a separate module from the types, so each carries the self-module type symbols
/// it names (its own type, plus any branded well-known type a decode `as`-cast
/// mentions) as references imported from `module`; the engine then redirects those
/// to an import of the types file.
pub fn emit_codecs(shape: &Shape, config: &CasingConfig, module: &str) -> Vec<Decl> {
    let decls = match &shape.kind {
        ShapeKind::Structure { members, .. } => struct_codecs(shape, members, config),
        ShapeKind::Enum { .. } => enum_codecs(shape),
        ShapeKind::Union {
            members,
            discriminator,
            ..
        } => union_codecs(shape, members, discriminator),
        _ => Vec::new(),
    };
    attach_type_refs(decls, shape, module)
}

/// Attach to every codec function the self-module types it references, so the serde
/// file imports them from the types file: the shape's own type and, for a structure
/// or union, any branded well-known type a member decode mentions.
fn attach_type_refs(decls: Vec<Decl>, shape: &Shape, module: &str) -> Vec<Decl> {
    let mut names: Vec<String> = vec![type_ident(shape, LANG)];
    if let ShapeKind::Structure { members, .. } | ShapeKind::Union { members, .. } = &shape.kind {
        for member in members {
            names.extend(branded_types(&member.target));
        }
    }
    let refs: Vec<Symbol> = names
        .into_iter()
        .map(|name| Symbol::imported(name.clone(), module.to_string(), name))
        .collect();
    decls
        .into_iter()
        .map(|decl| with_refs(decl, &refs))
        .collect()
}

/// The branded well-known type names a value of IR type `t` decodes into via an
/// `as`-cast, recursing through collections (its keys excluded — map keys stay
/// strings). Only these need importing for the decode annotations.
fn branded_types(t: &Tref) -> Vec<String> {
    match t {
        Tref::Prim(Prim::Timestamp) => vec!["Timestamp".into()],
        Tref::Prim(Prim::Date) => vec!["LocalDate".into()],
        Tref::Prim(Prim::Duration) => vec!["Duration".into()],
        Tref::List(inner) | Tref::Map(_, inner) => branded_types(inner),
        _ => Vec::new(),
    }
}

/// Add `refs` to a codec function's body so import collection reaches them. Only
/// functions carry a body; any other decl is returned unchanged.
fn with_refs(decl: Decl, refs: &[Symbol]) -> Decl {
    match decl {
        Decl::Function(Function {
            name,
            params,
            ret,
            body:
                FnBody::Raw {
                    text,
                    refs: mut existing,
                },
        }) => {
            existing.extend(refs.iter().cloned());
            Decl::Function(Function {
                name,
                params,
                ret,
                body: FnBody::Raw {
                    text,
                    refs: existing,
                },
            })
        }
        other => other,
    }
}

fn union_codecs(shape: &Shape, members: &[Member], discriminator: &str) -> Vec<Decl> {
    let ty = type_ident(shape, LANG);
    let case = |op: &str, src: &str, suffix: &str| -> String {
        members
            .iter()
            .map(|m| {
                let tag = wire_key(m);
                let payload = payload_codec_name(&m.target);
                // The payload codec returns `unknown` on encode, so the spread is
                // cast to an object type to remain valid under strict TypeScript.
                format!(
                    "    case \"{tag}\": return {{ {discriminator}: \"{tag}\", ...({op}{payload}({src}) as object) }}{suffix};\n"
                )
            })
            .collect()
    };
    let encode_body = format!(
        "  switch (value.{discriminator}) {{\n{}  }}\n  throw new Error(\"unknown variant\");",
        case("encode", "value as any", "")
    );
    let decode_body = format!(
        "  switch (raw.{discriminator}) {{\n{}  }}\n  throw new Error(\"unknown variant\");",
        case("decode", "raw", &format!(" as {ty}"))
    );
    vec![
        function_owned(
            &format!("encode{ty}"),
            &[("value", &ty)],
            "unknown",
            encode_body,
        ),
        function_owned(&format!("decode{ty}"), &[("raw", "any")], &ty, decode_body),
    ]
}

/// The codec suffix for a variant's payload type. Variant payloads are
/// references in practice; a non-reference payload has no codec.
fn payload_codec_name(target: &Tref) -> String {
    match target {
        Tref::Ref { id, .. } => type_suffix(id),
        _ => String::new(),
    }
}

fn struct_codecs(shape: &Shape, members: &[Member], config: &CasingConfig) -> Vec<Decl> {
    let ty = type_ident(shape, LANG);
    let encode_fields: String = members
        .iter()
        .map(|m| {
            let access = format!("value.{}", field_ident(m, config, LANG));
            let expr = guard_null(m, &access, member_encode(&access, m));
            format!("    {}: {expr},\n", wire_key(m))
        })
        .collect();
    let decode_fields: String = members
        .iter()
        .map(|m| {
            let access = format!("raw.{}", wire_key(m));
            let expr = guard_null(m, &access, member_decode(&access, m));
            format!("    {}: {expr},\n", field_ident(m, config, LANG))
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

fn enum_codecs(shape: &Shape) -> Vec<Decl> {
    // An open enum is a string on the wire; encode is identity and decode is a
    // lenient cast that lets an unknown value pass through.
    let ty = type_ident(shape, LANG);
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

/// The encode expression for a member, taking the `@entries` escape into account:
/// an entries map is an array of `[k, v]` pairs that is encoded element-wise.
fn member_encode(access: &str, member: &Member) -> String {
    match (&member.target, has_entries(&member.traits)) {
        (Tref::Map(k, v), true) => format!(
            "{access}.map(([k, v]) => [{}, {}])",
            encode_expr("k", k),
            encode_expr("v", v)
        ),
        _ => encode_expr(access, &member.target),
    }
}

/// The decode expression for a member, taking the `@entries` escape into account.
fn member_decode(access: &str, member: &Member) -> String {
    match (&member.target, has_entries(&member.traits)) {
        (Tref::Map(k, v), true) => format!(
            "{access}.map(([k, v]: [any, any]) => [{}, {}])",
            decode_expr("k", k),
            decode_expr("v", v)
        ),
        _ => decode_expr(access, &member.target),
    }
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
fn encode_expr(value: &str, t: &Tref) -> String {
    match t {
        Tref::Prim(Prim::I64 | Prim::U64) => format!("encodeI64({value})"),
        Tref::Prim(Prim::Bytes) => format!("encodeBytes({value})"),
        // Native types and branded strings are already wire-shaped.
        Tref::Prim(_) | Tref::Param(_) => value.to_string(),
        Tref::Ref { id, .. } => format!("encode{}({value})", type_suffix(id)),
        Tref::List(inner) => format!("{value}.map((x) => {})", encode_expr("x", inner)),
        Tref::Map(_, v) => format!(
            "Object.fromEntries(Object.entries({value}).map(([k, v]) => [k, {}]))",
            encode_expr("v", v)
        ),
    }
}

/// The TypeScript expression that decodes `value` of IR type `t` from its wire
/// form into the in-memory representation.
fn decode_expr(value: &str, t: &Tref) -> String {
    match t {
        Tref::Prim(Prim::I64 | Prim::U64) => format!("decodeI64({value})"),
        Tref::Prim(Prim::Bytes) => format!("decodeBytes({value})"),
        Tref::Prim(Prim::Timestamp) => format!("({value} as Timestamp)"),
        Tref::Prim(Prim::Date) => format!("({value} as LocalDate)"),
        Tref::Prim(Prim::Duration) => format!("({value} as Duration)"),
        Tref::Prim(_) | Tref::Param(_) => value.to_string(),
        Tref::Ref { id, .. } => format!("decode{}({value})", type_suffix(id)),
        Tref::List(inner) => {
            format!("{value}.map((x: any) => {})", decode_expr("x", inner))
        }
        Tref::Map(_, v) => format!(
            "Object.fromEntries(Object.entries({value}).map(([k, v]: [string, any]) => [k, {}]))",
            decode_expr("v", v)
        ),
    }
}

/// The codec suffix for a referenced type: its PascalCase name after `module#`,
/// used as-is (type names are PascalCase in the IR). A referenced type's own
/// `@rename` is not visible here.
fn type_suffix(id: &str) -> String {
    crate::codegen::conventions::type_ident_from_id(id)
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
    use crate::codegen::test_support::{enum_shape, member, structure, union_shape};

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
        let shape = structure(
            "billing#Charge",
            vec![
                member("amount_cents", Tref::Prim(Prim::I64), true),
                member("note", Tref::Prim(Prim::String), false),
            ],
        );
        let out = rendered(&emit_codecs(&shape, &ts_casing(), "billing"));
        assert!(out.contains("export function encodeCharge(value: Charge): unknown {"));
        // encode: wire key out, in-code identifier read, i64 routed.
        assert!(out.contains("amount_cents: encodeI64(value.amountCents),"));
        // optional field omitted when null.
        assert!(out.contains("note: value.note == null ? undefined : value.note,"));
        assert!(out.contains("export function decodeCharge(raw: any): Charge {"));
        assert!(out.contains("amountCents: decodeI64(raw.amount_cents),"));
    }

    #[test]
    fn an_entries_field_encodes_and_decodes_element_wise() {
        let mut counts = member(
            "counts",
            Tref::Map(
                Box::new(Tref::Prim(Prim::I32)),
                Box::new(Tref::Prim(Prim::I64)),
            ),
            true,
        );
        counts.traits = vec![crate::ir::Trait {
            id: "core#entries".into(),
            value: serde_json::json!(true),
        }];
        let shape = structure("billing#Doc", vec![counts]);
        let out = rendered(&emit_codecs(&shape, &ts_casing(), "billing"));
        // The pairs array is mapped element-wise; the i64 value routes through its
        // codec, the i32 key passes through.
        assert!(out.contains("counts: value.counts.map(([k, v]) => [k, encodeI64(v)]),"));
        assert!(out.contains("counts: raw.counts.map(([k, v]: [any, any]) => [k, decodeI64(v)]),"));
    }

    #[test]
    fn open_enum_codec_is_identity_and_lenient() {
        let shape = enum_shape("billing#Status", vec![("pending".into(), None)]);
        let out = rendered(&emit_codecs(&shape, &ts_casing(), "billing"));
        assert!(out.contains("export function encodeStatus(value: Status): string {"));
        assert!(out.contains("return value;"));
        assert!(out.contains("export function decodeStatus(raw: string): Status {"));
        assert!(out.contains("return raw as Status;"));
    }

    #[test]
    fn union_codec_switches_on_the_discriminator() {
        let shape = union_shape(
            "billing#payment_method",
            "type",
            vec![member(
                "card",
                Tref::Ref {
                    id: "billing#card_data".into(),
                    args: vec![],
                },
                true,
            )],
        );
        let out = rendered(&emit_codecs(&shape, &ts_casing(), "billing"));
        assert!(
            out.contains("export function encodePaymentMethod(value: PaymentMethod): unknown {")
        );
        assert!(out.contains("switch (value.type) {"));
        assert!(out.contains(
            "case \"card\": return { type: \"card\", ...(encodeCardData(value as any) as object) };"
        ));
        assert!(out.contains("export function decodePaymentMethod(raw: any): PaymentMethod {"));
        assert!(out.contains(
            "case \"card\": return { type: \"card\", ...(decodeCardData(raw) as object) } as PaymentMethod;"
        ));
        assert!(out.contains("throw new Error(\"unknown variant\");"));
    }

    #[test]
    fn a_non_reference_variant_payload_has_no_codec_name() {
        // Defensive: variant payloads are references in practice.
        assert_eq!(payload_codec_name(&Tref::Prim(Prim::Bool)), "");
    }

    #[test]
    fn unsupported_shapes_emit_no_codecs() {
        let service = Shape {
            id: "billing#Api".into(),
            kind: ShapeKind::Service { operations: vec![] },
            traits: vec![],
        };
        assert!(emit_codecs(&service, &ts_casing(), "billing").is_empty());
    }

    #[test]
    fn expressions_compose_over_collections_refs_and_branded() {
        let card = || Tref::Ref {
            id: "m#Charge".into(),
            args: vec![],
        };
        let bytes_map = || {
            Tref::Map(
                Box::new(Tref::Prim(Prim::String)),
                Box::new(Tref::Prim(Prim::Bytes)),
            )
        };
        // bytes, branded, ref, list, map, param, native.
        assert_eq!(encode_expr("x", &Tref::Prim(Prim::Bytes)), "encodeBytes(x)");
        assert_eq!(encode_expr("x", &Tref::Prim(Prim::Bool)), "x");
        assert_eq!(encode_expr("x", &Tref::Param("T".into())), "x");
        assert_eq!(
            decode_expr("x", &Tref::Prim(Prim::Timestamp)),
            "(x as Timestamp)"
        );
        assert_eq!(
            decode_expr("x", &Tref::Prim(Prim::Date)),
            "(x as LocalDate)"
        );
        assert_eq!(
            decode_expr("x", &Tref::Prim(Prim::Duration)),
            "(x as Duration)"
        );
        // uuid is not branded: it decodes as a plain string, untouched.
        assert_eq!(decode_expr("x", &Tref::Prim(Prim::Uuid)), "x");
        assert_eq!(encode_expr("x", &card()), "encodeCharge(x)");
        assert_eq!(decode_expr("x", &card()), "decodeCharge(x)");
        assert_eq!(
            encode_expr("xs", &Tref::List(Box::new(Tref::Prim(Prim::I64)))),
            "xs.map((x) => encodeI64(x))"
        );
        assert_eq!(
            decode_expr("xs", &Tref::List(Box::new(Tref::Prim(Prim::I64)))),
            "xs.map((x: any) => decodeI64(x))"
        );
        assert!(encode_expr("m", &bytes_map()).contains("encodeBytes(v)"));
        assert!(decode_expr("m", &bytes_map()).contains("decodeBytes(v)"));
    }
}
