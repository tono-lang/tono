//! Generated Go codecs: the small amount of custom marshaling `encoding/json`
//! cannot express on its own. Struct fields carry json tags, so the standard
//! library handles scalars, optionals, bytes, named-string enums, and well-known
//! types natively; only two cases need code here.
//!
//! - A union is an interface with one wrapper struct per variant. Each wrapper has
//!   a `MarshalJSON` that flattens its payload and injects the discriminator (via
//!   the shared `marshalVariant` helper); a free `unmarshalX` peeks the
//!   discriminator and dispatches. A struct that holds a union field gets a thin
//!   `UnmarshalJSON` that decodes that one field through `unmarshalX`, leaving the
//!   rest to `encoding/json` via an embedded alias.
//! - An `@entries` map is the generic `Entries[K, V]`, whose `MarshalJSON` /
//!   `UnmarshalJSON` carry the pairs-array wire shape.
//!
//! Everything else — including the union container's marshal direction, which uses
//! the dynamic value's own `MarshalJSON` — is left to `encoding/json`.

use crate::codegen::casing::{transform, CaseStyle, CasingConfig};
use crate::codegen::conventions::{field_ident, type_ident, type_ident_from_id, wire_key};
use crate::codegen::symbol::{Symbol, SymbolKind};
use crate::codegen::targets::go::symbols::symbol_of;
use crate::codegen::tree::{Decl, Raw};
use crate::ir::{Member, Shape, ShapeKind, Tref};
use std::collections::HashSet;

/// The Go language key for per-language traits such as `@rename`.
const LANG: &str = "go";

/// Which shared runtime helpers a file needs. `entries` pulls the generic
/// `Entry`/`Entries` types; `variant` pulls `marshalVariant`. Each is emitted only
/// when some shape in the file uses it, so a tags-only file imports nothing.
#[derive(Clone, Copy)]
pub struct RuntimeHelpers {
    pub entries: bool,
    pub variant: bool,
}

/// A `Decl::Raw` carrying the given Go source and the symbols it references (so the
/// engine still collects their imports).
fn raw(text: impl Into<String>, refs: Vec<Symbol>) -> Decl {
    Decl::Raw(Raw {
        text: text.into(),
        refs,
    })
}

/// The `encoding/json` import symbol, referenced by every generated codec.
fn json_ref() -> Symbol {
    Symbol::imported("json", "encoding/json", "json")
}

/// The `fmt` import symbol, referenced where a generated codec wraps an error.
fn fmt_ref() -> Symbol {
    Symbol::imported("fmt", "fmt", "fmt")
}

/// The shared runtime helpers a generated file relies on, emitted once per file
/// and only for the cases the file actually uses. `Entries[K, V]` carries the
/// pairs-array wire shape; `marshalVariant` flattens a union payload and injects
/// its discriminator. Both reference `encoding/json`; `marshalVariant` is the only
/// `fmt` user among them — none here, since the variant error wrapping lives in the
/// generated `unmarshalX`.
pub fn runtime_helpers(helpers: RuntimeHelpers) -> Vec<Decl> {
    let mut decls = Vec::new();
    if helpers.entries {
        let text = "\
type Entry[K comparable, V any] struct {
\tKey   K
\tValue V
}
type Entries[K comparable, V any] []Entry[K, V]

func (e Entries[K, V]) MarshalJSON() ([]byte, error) {
\tpairs := make([][2]any, len(e))
\tfor i, en := range e {
\t\tpairs[i] = [2]any{en.Key, en.Value}
\t}
\treturn json.Marshal(pairs)
}

func (e *Entries[K, V]) UnmarshalJSON(b []byte) error {
\tvar pairs [][2]json.RawMessage
\tif err := json.Unmarshal(b, &pairs); err != nil {
\t\treturn err
\t}
\tout := make(Entries[K, V], len(pairs))
\tfor i, p := range pairs {
\t\tvar k K
\t\tvar v V
\t\tif err := json.Unmarshal(p[0], &k); err != nil {
\t\t\treturn err
\t\t}
\t\tif err := json.Unmarshal(p[1], &v); err != nil {
\t\t\treturn err
\t\t}
\t\tout[i] = Entry[K, V]{Key: k, Value: v}
\t}
\t*e = out
\treturn nil
}";
        decls.push(raw(text, vec![json_ref()]));
    }
    if helpers.variant {
        let text = "\
func marshalVariant(payload any, disc, tag string) ([]byte, error) {
\tb, err := json.Marshal(payload)
\tif err != nil {
\t\treturn nil, err
\t}
\tvar obj map[string]json.RawMessage
\tif err := json.Unmarshal(b, &obj); err != nil {
\t\treturn nil, err
\t}
\tobj[disc], _ = json.Marshal(tag)
\treturn json.Marshal(obj)
}";
        decls.push(raw(text, vec![json_ref()]));
    }
    decls
}

/// Emit the codecs for a shape: a union's interface, wrappers, and dispatcher; a
/// struct's `UnmarshalJSON` when it holds a union field. Every other shape (enum,
/// well-known, plain struct) is handled entirely by `encoding/json` tags and emits
/// nothing. `unions` is the set of union type identifiers in the module, used to
/// detect a union-typed struct field.
pub fn emit_codecs(shape: &Shape, config: &CasingConfig, unions: &HashSet<String>) -> Vec<Decl> {
    match &shape.kind {
        ShapeKind::Structure { members, .. } => struct_codecs(shape, members, config, unions),
        ShapeKind::Union {
            members,
            discriminator,
            ..
        } => union_codecs(shape, members, discriminator),
        _ => Vec::new(),
    }
}

/// The union type identifier a member's type refers to, if any. A union field is a
/// nominal reference whose target type is one of the module's unions.
fn union_of(target: &Tref, unions: &HashSet<String>) -> Option<String> {
    match target {
        Tref::Ref { id, .. } => {
            let ident = type_ident_from_id(id);
            unions.contains(&ident).then_some(ident)
        }
        _ => None,
    }
}

/// The `UnmarshalJSON` a struct needs when it holds a union field: `encoding/json`
/// cannot decode into an interface, so the method decodes the whole struct through
/// an embedded alias (so every other field rides the standard library) and
/// re-reads each union field as a `json.RawMessage`, dispatching it through the
/// union's `unmarshalX`. A struct with no union field needs no method.
fn struct_codecs(
    shape: &Shape,
    members: &[Member],
    config: &CasingConfig,
    unions: &HashSet<String>,
) -> Vec<Decl> {
    // Each union field carries its in-code identifier, its wire key (so the alias
    // override shadows the same json tag), and the union it dispatches through.
    let union_fields: Vec<(String, String, String)> = members
        .iter()
        .filter_map(|m| {
            union_of(&m.target, unions)
                .map(|union| (field_ident(m, config, LANG), wire_key(m), union))
        })
        .collect();
    if union_fields.is_empty() {
        return Vec::new();
    }
    let ty = type_ident(shape, LANG);
    let recv = ty.chars().next().unwrap_or('a').to_ascii_lowercase();

    // The embedded alias decodes every non-union field; each union field is shadowed
    // by a `json.RawMessage` so it can be dispatched after.
    let mut overrides = String::new();
    for (field, wire, _) in &union_fields {
        overrides.push_str(&format!("\t\t{field} json.RawMessage `json:\"{wire}\"`\n",));
    }
    let mut dispatch = String::new();
    for (field, _, union) in &union_fields {
        let unmarshal = format!("unmarshal{union}");
        dispatch.push_str(&format!(
            "\tif len(tmp.{field}) > 0 {{\n\t\tm, err := {unmarshal}(tmp.{field})\n\t\tif err != nil {{\n\t\t\treturn err\n\t\t}}\n\t\t{recv}.{field} = m\n\t}}\n",
        ));
    }
    let text = format!(
        "func ({recv} *{ty}) UnmarshalJSON(b []byte) error {{\n\
         \ttype alias {ty}\n\
         \tvar tmp struct {{\n\
         \t\talias\n\
         {overrides}\t}}\n\
         \ttmp.alias = alias(*{recv})\n\
         \tif err := json.Unmarshal(b, &tmp); err != nil {{\n\
         \t\treturn err\n\
         \t}}\n\
         \t*{recv} = {ty}(tmp.alias)\n\
         {dispatch}\treturn nil\n\
         }}",
    );
    vec![raw(text, vec![json_ref()])]
}

/// Emit a union's interface, its variant wrappers (each with a `MarshalJSON` that
/// flattens its payload and injects the discriminator), and the free `unmarshalX`
/// that peeks the discriminator and decodes the matching payload. Marshal needs no
/// container method: a struct field typed as the interface marshals through the
/// dynamic value's own `MarshalJSON`.
fn union_codecs(shape: &Shape, members: &[Member], discriminator: &str) -> Vec<Decl> {
    let ty = type_ident(shape, LANG);
    let pascal = CasingConfig::new(CaseStyle::Pascal);
    let variant_ident = |m: &Member| transform(&m.name, SymbolKind::Variant, &pascal, None);
    let payload_ty = |m: &Member| symbol_of(&m.target).name;
    let marker = format!("is{ty}");

    // The interface and one wrapper struct per variant, each with its marker method
    // and a `MarshalJSON` that injects the discriminator.
    let mut iface = format!("type {ty} interface{{ {marker}() }}\n");
    for m in members {
        let wrapper = format!("{ty}{}", variant_ident(m));
        let payload = payload_ty(m);
        let tag = wire_key(m);
        iface.push_str(&format!(
            "\ntype {wrapper} struct{{ Value {payload} }}\n\n\
             func ({wrapper}) {marker}() {{}}\n\n\
             func (m {wrapper}) MarshalJSON() ([]byte, error) {{ return marshalVariant(m.Value, \"{discriminator}\", \"{tag}\") }}\n",
        ));
    }
    let mut iface_refs = vec![json_ref()];
    iface_refs.extend(members.iter().map(|m| symbol_of(&m.target)));

    // unmarshalX: peek the discriminator, decode the matching payload into its
    // wrapper. The payload decode reuses the struct's own json tags.
    let mut decode = format!(
        "func unmarshal{ty}(b []byte) ({ty}, error) {{\n\
         \tvar d map[string]json.RawMessage\n\
         \tif err := json.Unmarshal(b, &d); err != nil {{\n\
         \t\treturn nil, err\n\
         \t}}\n\
         \tvar tag string\n\
         \tjson.Unmarshal(d[\"{discriminator}\"], &tag)\n\
         \tswitch tag {{\n",
    );
    for m in members {
        let wrapper = format!("{ty}{}", variant_ident(m));
        let payload = payload_ty(m);
        let tag = wire_key(m);
        decode.push_str(&format!(
            "\tcase \"{tag}\":\n\t\tvar p {payload}\n\t\tif err := json.Unmarshal(b, &p); err != nil {{\n\t\t\treturn nil, err\n\t\t}}\n\t\treturn {wrapper}{{Value: p}}, nil\n",
        ));
    }
    decode.push_str("\t}\n\treturn nil, fmt.Errorf(\"unknown variant %q\", tag)\n}");

    let mut decode_refs = vec![json_ref(), fmt_ref()];
    decode_refs.extend(members.iter().map(|m| symbol_of(&m.target)));

    vec![raw(iface, iface_refs), raw(decode, decode_refs)]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codegen::target::RenderRules;
    use crate::codegen::targets::go::types::go_casing;
    use crate::codegen::targets::go::GoRules;
    use crate::codegen::test_support::{enum_shape, member, structure, union_shape};
    use crate::ir::{Prim, ShapeKind};

    fn rendered(decls: &[Decl]) -> String {
        decls
            .iter()
            .map(|d| GoRules.render_decl(d))
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn no_unions() -> HashSet<String> {
        HashSet::new()
    }

    #[test]
    fn entries_helper_carries_the_pairs_array_round_trip() {
        let out = rendered(&runtime_helpers(RuntimeHelpers {
            entries: true,
            variant: false,
        }));
        assert!(out.contains("type Entry[K comparable, V any] struct {"));
        assert!(out.contains("type Entries[K comparable, V any] []Entry[K, V]"));
        assert!(out.contains("func (e Entries[K, V]) MarshalJSON() ([]byte, error) {"));
        assert!(out.contains("func (e *Entries[K, V]) UnmarshalJSON(b []byte) error {"));
        assert!(out.contains("pairs[i] = [2]any{en.Key, en.Value}"));
        // No marshalVariant when only entries are requested.
        assert!(!out.contains("func marshalVariant("));
        let Decl::Raw(raw) = &runtime_helpers(RuntimeHelpers {
            entries: true,
            variant: false,
        })[0] else {
            panic!("the entries helper is a Raw decl");
        };
        assert!(raw.refs.iter().any(|s| s.name == "json"));
    }

    #[test]
    fn variant_helper_flattens_and_injects_the_discriminator() {
        let out = rendered(&runtime_helpers(RuntimeHelpers {
            entries: false,
            variant: true,
        }));
        assert!(
            out.contains("func marshalVariant(payload any, disc, tag string) ([]byte, error) {")
        );
        assert!(out.contains("obj[disc], _ = json.Marshal(tag)"));
        assert!(!out.contains("type Entries["));
    }

    #[test]
    fn no_helpers_requested_emits_nothing() {
        assert!(runtime_helpers(RuntimeHelpers {
            entries: false,
            variant: false,
        })
        .is_empty());
    }

    #[test]
    fn a_plain_struct_emits_no_codecs() {
        // Tags do all the work for a struct with no union field.
        let shape = structure(
            "billing#Charge",
            vec![
                member("amount_cents", Tref::Prim(Prim::I64), true),
                member("note", Tref::Prim(Prim::String), false),
            ],
        );
        assert!(emit_codecs(&shape, &go_casing(), &no_unions()).is_empty());
    }

    #[test]
    fn an_enum_emits_no_codecs() {
        let shape = enum_shape("billing#Status", vec![("pending".into(), None)]);
        assert!(emit_codecs(&shape, &go_casing(), &no_unions()).is_empty());
    }

    #[test]
    fn union_emits_interface_wrappers_marshalers_and_a_dispatcher() {
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
        let out = rendered(&emit_codecs(&shape, &go_casing(), &no_unions()));
        // The interface with one marker method and a wrapper per variant.
        assert!(out.contains("type PaymentMethod interface{ isPaymentMethod() }"));
        assert!(out.contains("type PaymentMethodCard struct{ Value CardData }"));
        assert!(out.contains("func (PaymentMethodCard) isPaymentMethod() {}"));
        assert!(out.contains("type PaymentMethodBank struct{ Value BankAccount }"));
        // Each wrapper marshals by flattening its payload and injecting the tag.
        assert!(out.contains(
            "func (m PaymentMethodCard) MarshalJSON() ([]byte, error) { return marshalVariant(m.Value, \"kind\", \"card\") }"
        ));
        // The dispatcher peeks the discriminator and decodes the matching payload.
        assert!(out.contains("func unmarshalPaymentMethod(b []byte) (PaymentMethod, error) {"));
        assert!(out.contains("json.Unmarshal(d[\"kind\"], &tag)"));
        assert!(out.contains("case \"card\":"));
        assert!(out.contains("var p CardData"));
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
        let out = rendered(&emit_codecs(&shape, &go_casing(), &no_unions()));
        // The in-code wrapper keeps the variant name; the wire tag is the override.
        assert!(out.contains("type MethodCard struct{ Value CardData }"));
        assert!(out.contains("marshalVariant(m.Value, \"type\", \"CARD\")"));
        assert!(out.contains("case \"CARD\":"));
    }

    #[test]
    fn a_struct_with_a_union_field_gets_an_unmarshal_method() {
        let unions: HashSet<String> = ["Method".to_string()].into_iter().collect();
        let shape = structure(
            "billing#Account",
            vec![
                member("account_id", Tref::Prim(Prim::I64), true),
                member(
                    "method",
                    Tref::Ref {
                        id: "billing#method".into(),
                        args: vec![],
                    },
                    true,
                ),
            ],
        );
        let out = rendered(&emit_codecs(&shape, &go_casing(), &unions));
        // The container method decodes through an embedded alias, shadows the union
        // field as RawMessage, and dispatches it through the union's unmarshalX.
        assert!(out.contains("func (a *Account) UnmarshalJSON(b []byte) error {"));
        assert!(out.contains("type alias Account"));
        assert!(out.contains("Method json.RawMessage `json:\"method\"`"));
        assert!(out.contains("tmp.alias = alias(*a)"));
        assert!(out.contains("*a = Account(tmp.alias)"));
        assert!(out.contains("m, err := unmarshalMethod(tmp.Method)"));
        assert!(out.contains("a.Method = m"));
        // No custom Marshal: the interface field marshals through its dynamic value.
        assert!(!out.contains("func (a Account) MarshalJSON"));
    }

    #[test]
    fn a_union_field_with_a_wire_override_shadows_the_right_key() {
        let unions: HashSet<String> = ["Method".to_string()].into_iter().collect();
        let shape = structure(
            "billing#Account",
            vec![crate::codegen::test_support::member_with(
                "method",
                Tref::Ref {
                    id: "billing#method".into(),
                    args: vec![],
                },
                true,
                vec![crate::ir::Trait {
                    id: "core#wire".into(),
                    value: serde_json::json!("pay_method"),
                }],
            )],
        );
        let out = rendered(&emit_codecs(&shape, &go_casing(), &unions));
        assert!(out.contains("Method json.RawMessage `json:\"pay_method\"`"));
    }

    #[test]
    fn unsupported_shapes_emit_no_codecs() {
        let service = Shape {
            id: "billing#Api".into(),
            kind: ShapeKind::Service { operations: vec![] },
            traits: vec![],
        };
        assert!(emit_codecs(&service, &go_casing(), &no_unions()).is_empty());
    }

    #[test]
    fn union_of_resolves_only_known_unions() {
        let unions: HashSet<String> = ["Method".to_string()].into_iter().collect();
        let method = Tref::Ref {
            id: "billing#method".into(),
            args: vec![],
        };
        assert_eq!(union_of(&method, &unions).as_deref(), Some("Method"));
        let other = Tref::Ref {
            id: "billing#charge".into(),
            args: vec![],
        };
        assert_eq!(union_of(&other, &unions), None);
        assert_eq!(union_of(&Tref::Prim(Prim::Bool), &unions), None);
    }
}
