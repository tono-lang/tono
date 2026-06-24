//! Generated Go codecs: per-type `encode`/`decode` functions that bridge the
//! in-memory representation and the JSON wire form for the cases the standard
//! `encoding/json` round-trip cannot express on its own (i64/u64 as string, bytes
//! as base64, the internally-tagged union, and the `@entries` pairs array). The
//! generated types are kept clean — no json tags, no marshal methods — and all
//! wire knowledge lives here.
//!
//! An encoder produces a wire-shaped `any` (a `map[string]any`, slice, or scalar)
//! that the driver hands to `json.Marshal`; a decoder consumes the `any` produced
//! by `json.Unmarshal` and yields the in-code value (with an `error`). Encoders
//! emit wire keys; decoders read wire keys and produce in-code identifiers, which
//! is where the identifier/wire-key split materializes.

use crate::codegen::casing::{transform, CaseStyle, CasingConfig};
use crate::codegen::conventions::{
    field_ident, has_entries, type_ident, type_ident_from_id, wire_key,
};
use crate::codegen::symbol::{Symbol, SymbolKind};
use crate::codegen::syntax;
use crate::codegen::targets::go::render::GoRules;
use crate::codegen::targets::go::symbols::symbol_of;
use crate::codegen::targets::go::types::type_expr_of;
use crate::codegen::tree::{Decl, Raw};
use crate::ir::{Member, Prim, Shape, ShapeKind, Tref};

/// The Go language key for per-language traits such as `@rename`.
const LANG: &str = "go";

/// The stdlib packages every generated file's runtime helpers reference,
/// declared as symbols so the engine collects their imports.
fn stdlib_refs() -> Vec<Symbol> {
    [
        ("fmt", "fmt"),
        ("strconv", "strconv"),
        ("base64", "encoding/base64"),
    ]
    .iter()
    .map(|(name, module)| Symbol::imported(*name, *module, *name))
    .collect()
}

/// The shared runtime helpers a generated file relies on, emitted once per file.
/// They are zero-dependency beyond the Go standard library; the encoders build a
/// wire-shaped `any`, the decoders consume one. They are top-level functions, so
/// an unused helper is not a compile error in Go.
pub fn runtime_helpers() -> Vec<Decl> {
    let text = "\
type Entry[K any, V any] struct {
\tKey   K
\tValue V
}

func encodeI64(v int64) any { return strconv.FormatInt(v, 10) }

func encodeU64(v uint64) any { return strconv.FormatUint(v, 10) }

func encodeBytes(b []byte) any { return base64.StdEncoding.EncodeToString(b) }

func encodeSlice[T any](xs []T, elem func(T) any) any {
\tout := make([]any, 0, len(xs))
\tfor _, x := range xs {
\t\tout = append(out, elem(x))
\t}
\treturn out
}

func encodeMap[V any](m map[string]V, elem func(V) any) any {
\tout := make(map[string]any, len(m))
\tfor k, v := range m {
\t\tout[k] = elem(v)
\t}
\treturn out
}

func encodeEntries[K any, V any](es []Entry[K, V], ek func(K) any, ev func(V) any) any {
\tout := make([]any, 0, len(es))
\tfor _, e := range es {
\t\tout = append(out, []any{ek(e.Key), ev(e.Value)})
\t}
\treturn out
}

func asObject(v any) (map[string]any, error) {
\tm, ok := v.(map[string]any)
\tif !ok {
\t\treturn nil, fmt.Errorf(\"expected object, got %T\", v)
\t}
\treturn m, nil
}

func decodeString(v any) (string, error) {
\ts, ok := v.(string)
\tif !ok {
\t\treturn \"\", fmt.Errorf(\"expected string, got %T\", v)
\t}
\treturn s, nil
}

func decodeBool(v any) (bool, error) {
\tb, ok := v.(bool)
\tif !ok {
\t\treturn false, fmt.Errorf(\"expected bool, got %T\", v)
\t}
\treturn b, nil
}

func decodeFloat64(v any) (float64, error) {
\tf, ok := v.(float64)
\tif !ok {
\t\treturn 0, fmt.Errorf(\"expected number, got %T\", v)
\t}
\treturn f, nil
}

func decodeInt8(v any) (int8, error) { f, err := decodeFloat64(v); return int8(f), err }

func decodeInt16(v any) (int16, error) { f, err := decodeFloat64(v); return int16(f), err }

func decodeInt32(v any) (int32, error) { f, err := decodeFloat64(v); return int32(f), err }

func decodeUint8(v any) (uint8, error) { f, err := decodeFloat64(v); return uint8(f), err }

func decodeUint16(v any) (uint16, error) { f, err := decodeFloat64(v); return uint16(f), err }

func decodeUint32(v any) (uint32, error) { f, err := decodeFloat64(v); return uint32(f), err }

func decodeI64(v any) (int64, error) {
\ts, err := decodeString(v)
\tif err != nil {
\t\treturn 0, err
\t}
\treturn strconv.ParseInt(s, 10, 64)
}

func decodeU64(v any) (uint64, error) {
\ts, err := decodeString(v)
\tif err != nil {
\t\treturn 0, err
\t}
\treturn strconv.ParseUint(s, 10, 64)
}

func decodeBytes(v any) ([]byte, error) {
\ts, err := decodeString(v)
\tif err != nil {
\t\treturn nil, err
\t}
\treturn base64.StdEncoding.DecodeString(s)
}

func decodeSlice[T any](v any, elem func(any) (T, error)) ([]T, error) {
\tarr, ok := v.([]any)
\tif !ok {
\t\treturn nil, fmt.Errorf(\"expected array, got %T\", v)
\t}
\tout := make([]T, 0, len(arr))
\tfor _, e := range arr {
\t\tx, err := elem(e)
\t\tif err != nil {
\t\t\treturn nil, err
\t\t}
\t\tout = append(out, x)
\t}
\treturn out, nil
}

func decodeMap[V any](v any, elem func(any) (V, error)) (map[string]V, error) {
\tm, err := asObject(v)
\tif err != nil {
\t\treturn nil, err
\t}
\tout := make(map[string]V, len(m))
\tfor k, e := range m {
\t\tx, err := elem(e)
\t\tif err != nil {
\t\t\treturn nil, err
\t\t}
\t\tout[k] = x
\t}
\treturn out, nil
}

func decodeEntries[K comparable, V any](v any, ek func(any) (K, error), ev func(any) (V, error)) ([]Entry[K, V], error) {
\tarr, ok := v.([]any)
\tif !ok {
\t\treturn nil, fmt.Errorf(\"expected array, got %T\", v)
\t}
\tout := make([]Entry[K, V], 0, len(arr))
\tfor _, e := range arr {
\t\tpair, ok := e.([]any)
\t\tif !ok || len(pair) != 2 {
\t\t\treturn nil, fmt.Errorf(\"expected [k, v] pair, got %T\", e)
\t\t}
\t\tk, err := ek(pair[0])
\t\tif err != nil {
\t\t\treturn nil, err
\t\t}
\t\tval, err := ev(pair[1])
\t\tif err != nil {
\t\t\treturn nil, err
\t\t}
\t\tout = append(out, Entry[K, V]{Key: k, Value: val})
\t}
\treturn out, nil
}"
        .to_string();
    vec![Decl::Raw(Raw {
        text,
        refs: stdlib_refs(),
    })]
}

/// Emit the encode/decode codecs (and, for a union, its sealed-interface type) for
/// a shape.
pub fn emit_codecs(shape: &Shape, config: &CasingConfig) -> Vec<Decl> {
    match &shape.kind {
        ShapeKind::Structure { members, .. } => struct_codecs(shape, members, config),
        ShapeKind::Enum { .. } => enum_codecs(shape),
        ShapeKind::Union {
            members,
            discriminator,
            ..
        } => union_codecs(shape, members, discriminator),
        _ => Vec::new(),
    }
}

/// A `Decl::Raw` carrying the given Go source and the symbols it references (so
/// the engine still collects their imports).
fn raw(text: String, refs: Vec<Symbol>) -> Decl {
    Decl::Raw(Raw { text, refs })
}

/// The symbols a member's codecs reference: the payload type for a nominal ref,
/// plus the stdlib `fmt` used by the generated error wrapping. The leaf symbol of
/// a collection element is collected recursively.
fn member_refs(t: &Tref) -> Vec<Symbol> {
    match t {
        Tref::Ref { .. } => vec![symbol_of(t)],
        Tref::List(inner) => member_refs(inner),
        Tref::Map(_, v) => member_refs(v),
        _ => Vec::new(),
    }
}

fn struct_codecs(shape: &Shape, members: &[Member], config: &CasingConfig) -> Vec<Decl> {
    let ty = type_ident(shape, LANG);
    let mut refs = vec![Symbol::imported("fmt", "fmt", "fmt")];

    // encode: build a wire-keyed map, omitting an absent optional field.
    let mut encode = format!("func encode{ty}(v {ty}) any {{\n\tm := map[string]any{{}}\n");
    for m in members {
        let field = field_ident(m, config, LANG);
        let access = format!("v.{field}");
        let wire = wire_key(m);
        refs.extend(member_refs(&m.target));
        if m.required {
            let expr = member_encode(&access, m);
            encode.push_str(&format!("\tm[\"{wire}\"] = {expr}\n"));
        } else {
            // An optional scalar/reference is a pointer; encode the dereferenced
            // value only when present.
            let deref = format!("(*{access})");
            let expr = member_encode(&deref, m);
            encode.push_str(&format!(
                "\tif {access} != nil {{\n\t\tm[\"{wire}\"] = {expr}\n\t}}\n"
            ));
        }
    }
    encode.push_str("\treturn m\n}");

    // decode: read each wire key, set the in-code field when the key is present.
    let mut decode = format!(
        "func decode{ty}(raw any) ({ty}, error) {{\n\tm, err := asObject(raw)\n\tif err != nil {{\n\t\treturn {ty}{{}}, err\n\t}}\n\tvar out {ty}\n"
    );
    for m in members {
        let field = field_ident(m, config, LANG);
        let wire = wire_key(m);
        let expr = member_decode("x", m);
        decode.push_str(&format!("\tif x, ok := m[\"{wire}\"]; ok && x != nil {{\n"));
        if m.required {
            decode.push_str(&format!(
                "\t\tout.{field}, err = {expr}\n\t\tif err != nil {{\n\t\t\treturn {ty}{{}}, fmt.Errorf(\"{wire}: %w\", err)\n\t\t}}\n"
            ));
        } else {
            // An optional field is a pointer: decode into a local, then take its
            // address.
            decode.push_str(&format!(
                "\t\tval, err := {expr}\n\t\tif err != nil {{\n\t\t\treturn {ty}{{}}, fmt.Errorf(\"{wire}: %w\", err)\n\t\t}}\n\t\tout.{field} = &val\n"
            ));
        }
        decode.push_str("\t}\n");
    }
    decode.push_str("\treturn out, nil\n}");

    vec![raw(encode, refs.clone()), raw(decode, refs)]
}

fn enum_codecs(shape: &Shape) -> Vec<Decl> {
    // An open enum is a string on the wire: encode is identity, decode is a lenient
    // cast that lets an unknown value pass through.
    let ty = type_ident(shape, LANG);
    let encode = format!("func encode{ty}(v {ty}) any {{\n\treturn string(v)\n}}");
    let decode = format!(
        "func decode{ty}(raw any) ({ty}, error) {{\n\ts, err := decodeString(raw)\n\tif err != nil {{\n\t\treturn \"\", err\n\t}}\n\treturn {ty}(s), nil\n}}"
    );
    vec![raw(encode, vec![]), raw(decode, vec![])]
}

/// Emit the union's sealed interface (the clean type), then its codecs. The
/// interface has one unexported marker method; each variant is a wrapper struct
/// holding the payload as `Value`. The codecs switch on the wrapper / discriminator
/// and flatten the payload next to the discriminator key.
fn union_codecs(shape: &Shape, members: &[Member], discriminator: &str) -> Vec<Decl> {
    let ty = type_ident(shape, LANG);
    let pascal = CasingConfig::new(CaseStyle::Pascal);
    let variant_ident = |m: &Member| transform(&m.name, SymbolKind::Variant, &pascal, None);
    let payload_ty = |m: &Member| symbol_of(&m.target).name;
    let codec_suffix = |m: &Member| match &m.target {
        Tref::Ref { id, .. } => type_ident_from_id(id),
        _ => String::new(),
    };

    // The sealed interface and its variant wrappers (the clean type).
    let marker = format!("is{ty}");
    let mut iface = format!("type {ty} interface{{ {marker}() }}\n");
    for m in members {
        let wrapper = format!("{ty}{}", variant_ident(m));
        let payload = payload_ty(m);
        iface.push_str(&format!(
            "\ntype {wrapper} struct{{ Value {payload} }}\n\nfunc ({wrapper}) {marker}() {{}}\n"
        ));
    }
    let mut iface_refs = vec![];
    iface_refs.extend(members.iter().map(|m| symbol_of(&m.target)));

    // encode: switch on the concrete wrapper, flatten the payload, set the tag.
    let mut encode = format!("func encode{ty}(v {ty}) any {{\n\tswitch x := v.(type) {{\n");
    for m in members {
        let wrapper = format!("{ty}{}", variant_ident(m));
        let suffix = codec_suffix(m);
        let tag = wire_key(m);
        encode.push_str(&format!(
            "\tcase {wrapper}:\n\t\tm := encode{suffix}(x.Value).(map[string]any)\n\t\tm[\"{discriminator}\"] = \"{tag}\"\n\t\treturn m\n"
        ));
    }
    encode.push_str("\t}\n\treturn nil\n}");

    // decode: peek the discriminator, decode the matching payload into its wrapper.
    let mut decode = format!(
        "func decode{ty}(raw any) ({ty}, error) {{\n\tm, err := asObject(raw)\n\tif err != nil {{\n\t\treturn nil, err\n\t}}\n\ttag, err := decodeString(m[\"{discriminator}\"])\n\tif err != nil {{\n\t\treturn nil, err\n\t}}\n\tswitch tag {{\n"
    );
    for m in members {
        let wrapper = format!("{ty}{}", variant_ident(m));
        let suffix = codec_suffix(m);
        let tag = wire_key(m);
        decode.push_str(&format!(
            "\tcase \"{tag}\":\n\t\tp, err := decode{suffix}(raw)\n\t\tif err != nil {{\n\t\t\treturn nil, err\n\t\t}}\n\t\treturn {wrapper}{{Value: p}}, nil\n"
        ));
    }
    decode.push_str("\t}\n\treturn nil, fmt.Errorf(\"unknown variant %q\", tag)\n}");

    let mut codec_refs = vec![Symbol::imported("fmt", "fmt", "fmt")];
    codec_refs.extend(members.iter().map(|m| symbol_of(&m.target)));

    vec![
        raw(iface, iface_refs),
        raw(encode, codec_refs.clone()),
        raw(decode, codec_refs),
    ]
}

/// The encode expression for a member, taking the `@entries` escape into account:
/// an entries map is a `[]Entry[K, V]` encoded element-wise into a pairs array.
fn member_encode(access: &str, member: &Member) -> String {
    match (&member.target, has_entries(&member.traits)) {
        (Tref::Map(k, v), true) => format!(
            "encodeEntries({access}, func(k {}) any {{ return {} }}, func(v {}) any {{ return {} }})",
            go_type(k),
            encode_expr("k", k),
            go_type(v),
            encode_expr("v", v),
        ),
        _ => encode_expr(access, &member.target),
    }
}

/// The decode expression for a member, taking the `@entries` escape into account.
fn member_decode(access: &str, member: &Member) -> String {
    match (&member.target, has_entries(&member.traits)) {
        (Tref::Map(k, v), true) => format!(
            "decodeEntries({access}, func(e any) ({}, error) {{ return {} }}, func(e any) ({}, error) {{ return {} }})",
            go_type(k),
            decode_expr("e", k),
            go_type(v),
            decode_expr("e", v),
        ),
        _ => decode_expr(access, &member.target),
    }
}

/// The Go spelling of an IR type, used for closure parameter / return types.
fn go_type(t: &Tref) -> String {
    syntax::render_type(&type_expr_of(t), &GoRules)
}

/// The Go expression that encodes `value` of IR type `t` into its wire form. Only
/// the cases that differ from a JSON-native value pass through a codec.
fn encode_expr(value: &str, t: &Tref) -> String {
    match t {
        Tref::Prim(Prim::I64) => format!("encodeI64({value})"),
        Tref::Prim(Prim::U64) => format!("encodeU64({value})"),
        Tref::Prim(Prim::Bytes) => format!("encodeBytes({value})"),
        // Native scalars and branded strings are already wire-shaped.
        Tref::Prim(_) | Tref::Param(_) => value.to_string(),
        Tref::Ref { id, .. } => format!("encode{}({value})", type_ident_from_id(id)),
        Tref::List(inner) => format!(
            "encodeSlice({value}, func(x {}) any {{ return {} }})",
            go_type(inner),
            encode_expr("x", inner)
        ),
        Tref::Map(_, v) => format!(
            "encodeMap({value}, func(x {}) any {{ return {} }})",
            go_type(v),
            encode_expr("x", v)
        ),
    }
}

/// The Go expression that decodes `value` of IR type `t` from its wire form into
/// the in-memory representation. Decoders return `(T, error)`.
fn decode_expr(value: &str, t: &Tref) -> String {
    match t {
        Tref::Prim(Prim::Bool) => format!("decodeBool({value})"),
        Tref::Prim(Prim::String | Prim::Uuid) => format!("decodeString({value})"),
        Tref::Prim(Prim::Bytes) => format!("decodeBytes({value})"),
        Tref::Prim(Prim::I8) => format!("decodeInt8({value})"),
        Tref::Prim(Prim::I16) => format!("decodeInt16({value})"),
        Tref::Prim(Prim::I32) => format!("decodeInt32({value})"),
        Tref::Prim(Prim::I64) => format!("decodeI64({value})"),
        Tref::Prim(Prim::U8) => format!("decodeUint8({value})"),
        Tref::Prim(Prim::U16) => format!("decodeUint16({value})"),
        Tref::Prim(Prim::U32) => format!("decodeUint32({value})"),
        Tref::Prim(Prim::U64) => format!("decodeU64({value})"),
        Tref::Prim(Prim::Float) => format!("decodeFloat64({value})"),
        // The branded well-known types are strings underneath, cast back.
        Tref::Prim(Prim::Timestamp) => brand_decode(value, "Timestamp"),
        Tref::Prim(Prim::Date) => brand_decode(value, "LocalDate"),
        Tref::Prim(Prim::Duration) => brand_decode(value, "Duration"),
        Tref::Param(name) => format!("{value}.({name}), error(nil)"),
        Tref::Ref { id, .. } => format!("decode{}({value})", type_ident_from_id(id)),
        Tref::List(inner) => format!(
            "decodeSlice({value}, func(e any) ({}, error) {{ return {} }})",
            go_type(inner),
            decode_expr("e", inner)
        ),
        Tref::Map(_, v) => format!(
            "decodeMap({value}, func(e any) ({}, error) {{ return {} }})",
            go_type(v),
            decode_expr("e", v)
        ),
    }
}

/// Decode a branded well-known type: a string on the wire, cast to its named type.
fn brand_decode(value: &str, brand: &str) -> String {
    format!(
        "func() ({brand}, error) {{ s, err := decodeString({value}); return {brand}(s), err }}()"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codegen::target::RenderRules;
    use crate::codegen::targets::go::types::go_casing;
    use crate::codegen::targets::go::GoRules;
    use crate::codegen::test_support::{enum_shape, member, structure, union_shape};
    use crate::ir::Trait;

    fn rendered(decls: &[Decl]) -> String {
        decls
            .iter()
            .map(|d| GoRules.render_decl(d))
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn runtime_helpers_cover_the_wire_primitives() {
        let out = rendered(&runtime_helpers());
        assert!(out.contains("func encodeI64(v int64) any { return strconv.FormatInt(v, 10) }"));
        assert!(out.contains("func encodeBytes(b []byte) any"));
        assert!(out.contains("func decodeI64(v any) (int64, error)"));
        assert!(out.contains("func decodeBytes(v any) ([]byte, error)"));
        assert!(out.contains("func decodeSlice[T any]"));
        assert!(out.contains("func decodeEntries[K comparable, V any]"));
        assert!(out.contains("type Entry[K any, V any] struct {"));
        // The stdlib packages are declared as refs.
        let Decl::Raw(raw) = &runtime_helpers()[0] else {
            panic!("runtime helpers are a Raw decl");
        };
        assert!(raw.refs.iter().any(|s| s.name == "strconv"));
        assert!(raw.refs.iter().any(|s| s.name == "base64"));
    }

    #[test]
    fn struct_codec_routes_i64_uses_wire_keys_and_omits_optionals() {
        let shape = structure(
            "billing#Charge",
            vec![
                member("amount_cents", Tref::Prim(Prim::I64), true),
                member("note", Tref::Prim(Prim::String), false),
            ],
        );
        let out = rendered(&emit_codecs(&shape, &go_casing()));
        assert!(out.contains("func encodeCharge(v Charge) any {"));
        // encode: wire key out, in-code identifier read, i64 routed.
        assert!(out.contains("m[\"amount_cents\"] = encodeI64(v.AmountCents)"));
        // an optional field is encoded only when present (pointer non-nil).
        assert!(out.contains("if v.Note != nil {"));
        assert!(out.contains("m[\"note\"] = (*v.Note)"));
        assert!(out.contains("func decodeCharge(raw any) (Charge, error) {"));
        assert!(out.contains("out.AmountCents, err = decodeI64(x)"));
        // an optional field decodes into a local, then takes its address.
        assert!(out.contains("out.Note = &val"));
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
        counts.traits = vec![Trait {
            id: "core#entries".into(),
            value: serde_json::json!(true),
        }];
        let shape = structure("billing#Doc", vec![counts]);
        let out = rendered(&emit_codecs(&shape, &go_casing()));
        // The pairs are mapped element-wise; the i64 value routes through its codec,
        // the i32 key passes through.
        assert!(out.contains(
            "encodeEntries(v.Counts, func(k int32) any { return k }, func(v int64) any { return encodeI64(v) })"
        ));
        assert!(out.contains(
            "decodeEntries(x, func(e any) (int32, error) { return decodeInt32(e) }, func(e any) (int64, error) { return decodeI64(e) })"
        ));
    }

    #[test]
    fn open_enum_codec_is_identity_and_lenient() {
        let shape = enum_shape("billing#Status", vec![("pending".into(), None)]);
        let out = rendered(&emit_codecs(&shape, &go_casing()));
        assert!(out.contains("func encodeStatus(v Status) any {"));
        assert!(out.contains("return string(v)"));
        assert!(out.contains("func decodeStatus(raw any) (Status, error) {"));
        assert!(out.contains("return Status(s), nil"));
    }

    #[test]
    fn union_emits_a_sealed_interface_and_switching_codecs() {
        let shape = union_shape(
            "billing#payment_method",
            "kind",
            vec![
                member(
                    "card",
                    Tref::Ref {
                        id: "billing#card_data".into(),
                        args: vec![],
                    },
                    true,
                ),
                member(
                    "bank",
                    Tref::Ref {
                        id: "billing#bank_account".into(),
                        args: vec![],
                    },
                    true,
                ),
            ],
        );
        let out = rendered(&emit_codecs(&shape, &go_casing()));
        // The sealed interface with one marker method and a wrapper per variant.
        assert!(out.contains("type PaymentMethod interface{ isPaymentMethod() }"));
        assert!(out.contains("type PaymentMethodCard struct{ Value CardData }"));
        assert!(out.contains("func (PaymentMethodCard) isPaymentMethod() {}"));
        assert!(out.contains("type PaymentMethodBank struct{ Value BankAccount }"));
        // encode switches on the wrapper, flattens the payload, sets the tag.
        assert!(out.contains("func encodePaymentMethod(v PaymentMethod) any {"));
        assert!(out.contains("case PaymentMethodCard:"));
        assert!(out.contains("m := encodeCardData(x.Value).(map[string]any)"));
        assert!(out.contains("m[\"kind\"] = \"card\""));
        // decode peeks the discriminator and decodes the matching payload.
        assert!(out.contains("func decodePaymentMethod(raw any) (PaymentMethod, error) {"));
        assert!(out.contains("tag, err := decodeString(m[\"kind\"])"));
        assert!(out.contains("case \"card\":"));
        assert!(out.contains("p, err := decodeCardData(raw)"));
        assert!(out.contains("return PaymentMethodCard{Value: p}, nil"));
        assert!(out.contains("unknown variant %q"));
    }

    #[test]
    fn a_union_variant_honors_its_wire_override() {
        let shape = union_shape(
            "billing#method",
            "type",
            vec![crate::codegen::test_support::wire_member(
                "card",
                "billing#card_data",
                Some("CARD"),
            )],
        );
        let out = rendered(&emit_codecs(&shape, &go_casing()));
        // The in-code wrapper keeps the variant name; the wire tag is the override.
        assert!(out.contains("type MethodCard struct{ Value CardData }"));
        assert!(out.contains("m[\"type\"] = \"CARD\""));
        assert!(out.contains("case \"CARD\":"));
    }

    #[test]
    fn unsupported_shapes_emit_no_codecs() {
        let service = Shape {
            id: "billing#Api".into(),
            kind: ShapeKind::Service { operations: vec![] },
            traits: vec![],
        };
        assert!(emit_codecs(&service, &go_casing()).is_empty());
    }

    #[test]
    fn expressions_compose_over_collections_refs_and_branded() {
        let charge = || Tref::Ref {
            id: "m#Charge".into(),
            args: vec![],
        };
        assert_eq!(encode_expr("x", &Tref::Prim(Prim::Bytes)), "encodeBytes(x)");
        assert_eq!(encode_expr("x", &Tref::Prim(Prim::U64)), "encodeU64(x)");
        assert_eq!(encode_expr("x", &Tref::Prim(Prim::Bool)), "x");
        assert_eq!(encode_expr("x", &Tref::Param("T".into())), "x");
        assert_eq!(decode_expr("x", &Tref::Prim(Prim::Bool)), "decodeBool(x)");
        assert_eq!(decode_expr("x", &Tref::Prim(Prim::Uuid)), "decodeString(x)");
        assert_eq!(
            decode_expr("x", &Tref::Prim(Prim::Float)),
            "decodeFloat64(x)"
        );
        // Every narrow integer width routes to its own conversion helper.
        assert_eq!(decode_expr("x", &Tref::Prim(Prim::I8)), "decodeInt8(x)");
        assert_eq!(decode_expr("x", &Tref::Prim(Prim::I16)), "decodeInt16(x)");
        assert_eq!(decode_expr("x", &Tref::Prim(Prim::I32)), "decodeInt32(x)");
        assert_eq!(decode_expr("x", &Tref::Prim(Prim::U8)), "decodeUint8(x)");
        assert_eq!(decode_expr("x", &Tref::Prim(Prim::U16)), "decodeUint16(x)");
        assert_eq!(decode_expr("x", &Tref::Prim(Prim::U32)), "decodeUint32(x)");
        assert_eq!(decode_expr("x", &Tref::Prim(Prim::U64)), "decodeU64(x)");
        assert!(decode_expr("x", &Tref::Prim(Prim::Timestamp)).contains("Timestamp(s)"));
        assert!(decode_expr("x", &Tref::Prim(Prim::Date)).contains("LocalDate(s)"));
        assert!(decode_expr("x", &Tref::Prim(Prim::Duration)).contains("Duration(s)"));
        assert_eq!(encode_expr("x", &charge()), "encodeCharge(x)");
        assert_eq!(decode_expr("x", &charge()), "decodeCharge(x)");
        assert_eq!(
            encode_expr("xs", &Tref::List(Box::new(Tref::Prim(Prim::I64)))),
            "encodeSlice(xs, func(x int64) any { return encodeI64(x) })"
        );
        assert_eq!(
            decode_expr("xs", &Tref::List(Box::new(Tref::Prim(Prim::I64)))),
            "decodeSlice(xs, func(e any) (int64, error) { return decodeI64(e) })"
        );
        let bytes_map = Tref::Map(
            Box::new(Tref::Prim(Prim::String)),
            Box::new(Tref::Prim(Prim::Bytes)),
        );
        assert!(encode_expr("m", &bytes_map).contains("encodeBytes(x)"));
        assert!(decode_expr("m", &bytes_map).contains("decodeBytes(e)"));
    }

    #[test]
    fn a_param_decode_passes_through_with_a_type_assertion() {
        assert_eq!(
            decode_expr("x", &Tref::Param("T".into())),
            "x.(T), error(nil)"
        );
    }

    #[test]
    fn member_refs_collects_the_leaf_of_a_collection() {
        // A nominal ref contributes its symbol; a primitive contributes nothing; a
        // list/map recurses to its element leaf.
        let charge = Tref::Ref {
            id: "m#Charge".into(),
            args: vec![],
        };
        assert_eq!(member_refs(&charge)[0].name, "Charge");
        assert!(member_refs(&Tref::Prim(Prim::Bool)).is_empty());
        let list_of_charge = Tref::List(Box::new(charge.clone()));
        assert_eq!(member_refs(&list_of_charge)[0].name, "Charge");
        let map_of_charge = Tref::Map(Box::new(Tref::Prim(Prim::String)), Box::new(charge));
        assert_eq!(member_refs(&map_of_charge)[0].name, "Charge");
    }

    #[test]
    fn a_non_reference_union_payload_yields_no_codec_suffix() {
        // Defensive: a variant payload is a reference in practice, so a non-reference
        // payload has no payload codec to call.
        let shape = union_shape(
            "billing#flag",
            "type",
            vec![member("on", Tref::Prim(Prim::Bool), true)],
        );
        let out = rendered(&emit_codecs(&shape, &go_casing()));
        // The wrapper exists; encode/decode call the empty-suffix `encode`/`decode`.
        assert!(out.contains("type FlagOn struct{ Value bool }"));
        assert!(out.contains("m := encode(x.Value).(map[string]any)"));
        assert!(out.contains("p, err := decode(raw)"));
    }
}
