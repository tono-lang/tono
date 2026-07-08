//! Building the component tree from IR shapes for the Rust target: `emit_type`
//! plus the IR-to-`TypeExpr` conversion and the idiomatic casing.
//!
//! Structures become `struct` interfaces here. Enums and unions need custom
//! serde plumbing (a custom `Deserialize` for the open `Unknown` arm, a tagged
//! `enum` for unions) and are emitted as verbatim items by a later phase.

use crate::codegen::casing::{self, CaseStyle, CasingConfig};
use crate::codegen::conventions::{self, field_ident};
use crate::codegen::ops::error_names;
use crate::codegen::symbol::{Symbol, SymbolKind};
use crate::codegen::targets::rust::codecs::{enum_item, enum_serde_item, union_item};
use crate::codegen::targets::rust::symbols::symbol_of;
use crate::codegen::tree::{Decl, Field, TypeExpr};
use crate::codegen::validation::{self, Measure, ValSyntax};
use crate::ir::{Member, Shape, ShapeKind, Tref};

/// The Rust language key for per-language traits such as `@rename`.
pub(crate) const LANG: &str = "rust";

/// The idiomatic Rust casing: snake_case fields. Type names are PascalCase in the
/// IR and used as-is (not cased); only the field default matters here.
pub fn rust_casing() -> CasingConfig {
    CasingConfig::new(CaseStyle::Snake)
}

/// Convert an IR type reference into a component-tree type expression through the
/// Rust symbol table.
pub fn type_expr_of(t: &Tref) -> TypeExpr {
    conventions::type_expr_of(t, &symbol_of)
}

/// Emit the type declaration(s) for a shape, which belong in the types file:
/// structures become struct interfaces; the open enum becomes its bare data-enum
/// definition; a union becomes its `#[serde(tag)]` enum (serde derives ride on the
/// type, so it is whole here). The open enum's hand-written serde impls are emitted
/// separately by [`emit_serde`]. Other shape kinds contribute nothing.
pub fn emit_type(shape: &Shape, config: &CasingConfig) -> Vec<Decl> {
    conventions::emit_shape(
        shape,
        LANG,
        |m| field_of(m, config),
        // Rust's open enum is a hand-written data enum (custom serde); the types
        // file holds only its definition, not the impls.
        |backing, values, name, dep, doc| vec![enum_item(backing, values, name, dep, doc)],
        |discriminator, members, name, dep, doc| {
            vec![union_item(discriminator, members, name, dep, doc)]
        },
    )
}

/// Emit the serde declaration(s) for a shape, which belong in the serde file: an
/// open enum's `open_enum!` invocation, which expands to the hand-written
/// `Serialize`/`Deserialize`. A structure or union derives its serde on the type,
/// so it contributes nothing here.
pub fn emit_serde(shape: &Shape) -> Vec<Decl> {
    match &shape.kind {
        ShapeKind::Enum { backing, values } => {
            vec![enum_serde_item(
                backing,
                values,
                &conventions::type_ident(shape, LANG),
            )]
        }
        _ => Vec::new(),
    }
}

/// The Rust spelling of a length measure: a string counts code points, and a list
/// or byte buffer counts its `len()`.
struct RustVal;
impl ValSyntax for RustVal {
    fn length(&self, access: &str, measure: Measure) -> String {
        match measure {
            Measure::Chars => format!("{access}.chars().count()"),
            Measure::Elements | Measure::Bytes => format!("{access}.len()"),
        }
    }
    fn wide_suffix(&self) -> &str {
        // Rust holds 64-bit integers natively, so a bound needs no literal suffix.
        ""
    }
}

/// Emit the validator for a structure whose members carry `@range`/`@length`
/// constraints: an `impl` with a `validate(&self) -> Vec<Violation>` that collects
/// one violation per failed check. It belongs in the types file, next to the struct
/// and the `Violation` record it references. A shape with no lowerable constraint
/// (or a generic one, unmodeled here) emits nothing.
pub fn emit_validators(shape: &Shape, config: &CasingConfig) -> Vec<Decl> {
    let Some(lines) = validation::structure_guard_lines(shape, &RustVal, "self.", config, LANG)
    else {
        return Vec::new();
    };
    let violation = error_names().violation;
    let inner = validation::validator_body(
        &lines,
        "        let mut violations = Vec::new();\n",
        "        violations\n",
        |l| {
            format!(
                "        if {} {{\n            violations.push({violation} {{ field: {:?}.to_string(), constraint: {:?}.to_string(), message: {:?}.to_string() }});\n        }}\n",
                l.condition, l.field, l.constraint, l.message
            )
        },
    );
    let ty = conventions::type_ident(shape, LANG);
    vec![Decl::raw(format!(
        "impl {ty} {{\n    pub fn validate(&self) -> Vec<{violation}> {{\n{inner}    }}\n}}"
    ))]
}

/// The PascalCase Rust identifier for an open-enum or union variant, derived from
/// its wire value / member name. Independent of the wire string the codec emits.
pub(crate) fn variant_ident(name: &str, config: &CasingConfig) -> String {
    casing::transform(name, SymbolKind::Variant, config, None)
}

fn field_of(member: &Member, config: &CasingConfig) -> Field {
    Field {
        name: Symbol::builtin(field_ident(member, config, LANG)),
        ty: conventions::entries_or_map(type_expr_of(&member.target), &member.traits),
        nullable: !member.required,
        wire: conventions::wire_of(&member.traits),
        deprecated: conventions::deprecated_of(&member.traits),
        doc: conventions::doc_of(&member.traits),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codegen::target::RenderRules;
    use crate::codegen::targets::rust::RustRules;
    use crate::codegen::test_support::{
        enum_shape, int_enum_shape, member, member_constrained, structure, union_shape,
    };
    use crate::ir::{Constraint, Prim, ShapeKind};

    #[test]
    fn a_constrained_struct_emits_a_validate_impl_over_its_checks() {
        let shape = structure(
            "billing#charge",
            vec![
                member_constrained(
                    "amount",
                    Tref::Prim(Prim::I64),
                    vec![Constraint::Range {
                        min: Some(0.0),
                        max: None,
                        excl_min: false,
                        excl_max: false,
                    }],
                ),
                member_constrained(
                    "currency",
                    Tref::Prim(Prim::String),
                    vec![Constraint::Length {
                        min: Some(3),
                        max: Some(3),
                    }],
                ),
            ],
        );
        let out = RustRules.render_decl(&emit_validators(&shape, &rust_casing())[0]);
        assert!(out.contains("impl Charge {"));
        assert!(out.contains("pub fn validate(&self) -> Vec<Violation> {"));
        // The i64 bound needs no suffix; the message states the inclusive minimum.
        assert!(out.contains("if self.amount < 0 {"));
        assert!(out.contains(
            "violations.push(Violation { field: \"amount\".to_string(), constraint: \"range\".to_string(), message: \"amount must be >= 0\".to_string() });"
        ));
        // A string length counts code points and bounds both ends.
        assert!(out.contains("if self.currency.chars().count() < 3 {"));
        assert!(out.contains("if self.currency.chars().count() > 3 {"));
    }

    #[test]
    fn a_struct_without_constraints_emits_no_validator() {
        let shape = structure(
            "billing#note",
            vec![member("text", Tref::Prim(Prim::String), true)],
        );
        assert!(emit_validators(&shape, &rust_casing()).is_empty());
    }

    #[test]
    fn a_structure_becomes_a_struct_with_snake_fields() {
        let shape = structure(
            "billing#Charge",
            vec![
                member("amount_cents", Tref::Prim(Prim::I64), true),
                member("note", Tref::Prim(Prim::String), false),
            ],
        );
        let decls = emit_type(&shape, &rust_casing());
        assert!(matches!(&decls[..], [Decl::Interface(i)]
            if i.name.name == "Charge"
                && i.fields[0].name.name == "amount_cents"
                && !i.fields[0].nullable
                && i.fields[1].name.name == "note"
                && i.fields[1].nullable));
    }

    #[test]
    fn an_enum_becomes_a_verbatim_open_enum_item() {
        let shape = enum_shape("billing#Status", vec![("pending".into(), None)]);
        let decls = emit_type(&shape, &rust_casing());
        assert!(matches!(&decls[..], [Decl::Raw(r)]
            if r.text.contains("pub enum Status {") && r.text.contains("Unknown(String)")));
    }

    #[test]
    fn an_int_backed_enum_has_an_i64_definition_and_i64_serde() {
        let shape = int_enum_shape(
            "billing#http_code",
            vec![("ok".into(), Some(200)), ("error".into(), Some(500))],
        );
        // The types file holds the data enum with the i64 catch-all.
        let types = emit_type(&shape, &rust_casing());
        assert!(matches!(&types[..], [Decl::Raw(r)]
            if r.text.contains("pub enum HTTPCode {") && r.text.contains("Unknown(i64)")));
        // The serde file holds the i64 codec as an `open_enum!` invocation spelling
        // each known value as an i64 literal.
        let serde = emit_serde(&shape);
        assert!(matches!(&serde[..], [Decl::Raw(r)]
            if r.text.contains("open_enum!(HTTPCode: i64 {")
                && r.text.contains("Ok => 200i64,")));
    }

    #[test]
    fn a_union_becomes_a_verbatim_tagged_enum_item() {
        let shape = union_shape(
            "billing#Method",
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
        let decls = emit_type(&shape, &rust_casing());
        assert!(matches!(&decls[..], [Decl::Raw(r)]
            if r.text.contains("#[serde(tag = \"type\")]")
                && r.text.contains("Card(CardData)")));
    }

    #[test]
    fn services_and_operations_emit_nothing() {
        let service = Shape {
            id: "billing#Api".into(),
            kind: ShapeKind::Service { operations: vec![] },
            traits: vec![],
        };
        assert!(emit_type(&service, &rust_casing()).is_empty());
    }

    #[test]
    fn variant_ident_pascal_cases_wire_values() {
        let config = CasingConfig::new(CaseStyle::Pascal);
        assert_eq!(variant_ident("pending", &config), "Pending");
        assert_eq!(variant_ident("card_present", &config), "CardPresent");
    }
}
