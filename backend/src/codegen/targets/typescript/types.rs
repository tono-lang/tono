//! Building the component tree from IR shapes for the TypeScript target:
//! `emit_type` plus the IR-to-`TypeExpr` conversion and the idiomatic casing.

use crate::codegen::casing::{CaseStyle, CasingConfig};
use crate::codegen::conventions::{self, field_ident, wire_of};
use crate::codegen::ops::error_names;
use crate::codegen::symbol::Symbol;
use crate::codegen::targets::typescript::symbols::symbol_of;
use crate::codegen::tree::{Decl, Field, TypeExpr, UnionDecl, Variant};
use crate::codegen::validation::{self, Measure, ValSyntax};
use crate::ir::{Member, Shape, Tref};

/// The TypeScript language key for per-language traits such as `@rename`.
pub(crate) const LANG: &str = "typescript";

/// The idiomatic TypeScript casing: camelCase fields and methods. Type names are
/// PascalCase in the IR and used as-is (not cased), and enum members and variant
/// tags are wire values kept verbatim, so only the field/method default matters.
pub fn ts_casing() -> CasingConfig {
    CasingConfig::new(CaseStyle::Camel)
}

/// Convert an IR type reference into a component-tree type expression through the
/// TypeScript symbol table.
pub fn type_expr_of(t: &Tref) -> TypeExpr {
    conventions::type_expr_of(t, &symbol_of)
}

/// Emit the declaration(s) for a shape. Structures become interfaces; enums
/// become (open) literal-union types. Other shape kinds are handled by later
/// phases and emit nothing here.
pub fn emit_type(shape: &Shape, config: &CasingConfig) -> Vec<Decl> {
    conventions::emit_shape(
        shape,
        LANG,
        |m| field_of(m, config),
        // An open enum is a literal-union type built from its wire values: string
        // tags for a string-backed enum, integer literals for an int-backed one.
        |backing, values, name, dep, doc| {
            vec![conventions::open_enum(backing, values, name, dep, doc)]
        },
        |discriminator, members, name, dep, doc| {
            vec![Decl::Union(UnionDecl {
                name: Symbol::builtin(name.to_string()),
                discriminator: discriminator.to_string(),
                variants: members.iter().map(variant_of).collect(),
                deprecated: dep.map(str::to_string),
                doc: doc.map(str::to_string),
            })]
        },
    )
}

/// Build a union variant: the tag is the member name (overridable by `@wire`),
/// and the payload is the referenced type the discriminator object intersects
/// with. The tag is a wire value, kept verbatim rather than cased.
fn variant_of(member: &Member) -> Variant {
    Variant {
        name: Symbol::builtin(member.name.clone()),
        fields: Vec::new(),
        payload: Some(type_expr_of(&member.target)),
        wire: wire_of(&member.traits),
        doc: conventions::doc_of(&member.traits),
    }
}

fn field_of(member: &Member, config: &CasingConfig) -> Field {
    Field {
        name: Symbol::builtin(field_ident(member, config, LANG)),
        ty: conventions::entries_or_map(type_expr_of(&member.target), &member.traits),
        nullable: !member.required,
        wire: wire_of(&member.traits),
        deprecated: conventions::deprecated_of(&member.traits),
        doc: conventions::doc_of(&member.traits),
    }
}

/// The TypeScript spelling of a length measure: a string counts code points via
/// spread (so astral characters count once, unlike `.length`'s UTF-16 units), and
/// an array counts its `.length`.
pub(crate) struct TsVal;
impl ValSyntax for TsVal {
    fn length(&self, access: &str, measure: Measure) -> String {
        match measure {
            Measure::Chars => format!("[...{access}].length"),
            Measure::Elements | Measure::Bytes => format!("{access}.length"),
        }
    }
    fn wide_suffix(&self) -> &str {
        // A 64-bit integer is a `bigint`, whose literal carries the `n` suffix.
        "n"
    }
}

/// Emit the validator for a structure whose members carry `@range`/`@length`
/// constraints: a `validate<Type>(value: <Type>): ValidationError | null` that
/// collects one violation per failed check and returns the taxonomy's Validation
/// category when any check fails. It belongs in the types file, next to the
/// interface and the error classes it references. A shape with no lowerable
/// constraint (or a generic one, unmodeled here) emits nothing.
pub fn emit_validators(shape: &Shape, config: &CasingConfig) -> Vec<Decl> {
    let Some(lines) = validation::structure_guard_lines(shape, &TsVal, "value.", config, LANG)
    else {
        return Vec::new();
    };
    let n = error_names();
    let body = validation::validator_body(
        &lines,
        &format!("  const violations: {}[] = [];\n", n.violation),
        &format!(
            "  return violations.length > 0 ? new {}(violations) : null;",
            n.validation
        ),
        |l| {
            format!(
                "  if ({}) {{\n    violations.push({{ field: {:?}, constraint: {:?}, message: {:?} }});\n  }}\n",
                l.condition, l.field, l.constraint, l.message
            )
        },
    );
    let ty = conventions::type_ident(shape, LANG);
    vec![validation::validator_function(
        format!("validate{ty}"),
        ty.clone(),
        TypeExpr::nullable(TypeExpr::Ref(Symbol::builtin(n.validation))),
        body,
    )]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codegen::target::RenderRules;
    use crate::codegen::targets::typescript::TsRules;
    use crate::codegen::test_support::{
        enum_shape, int_enum_shape, member, member_constrained, structure, union_shape,
    };
    use crate::codegen::tree::EnumRepr;
    use crate::ir::Constraint;

    #[test]
    fn a_constrained_struct_emits_a_validate_function_over_its_checks() {
        let shape = structure(
            "billing#charge",
            vec![
                member_constrained(
                    "amount",
                    Tref::Prim(crate::ir::Prim::I64),
                    vec![Constraint::Range {
                        min: Some(0.0),
                        max: None,
                        excl_min: false,
                        excl_max: false,
                    }],
                ),
                member_constrained(
                    "currency",
                    Tref::Prim(crate::ir::Prim::String),
                    vec![Constraint::Length {
                        min: Some(3),
                        max: Some(3),
                    }],
                ),
            ],
        );
        let out = TsRules.render_decl(&emit_validators(&shape, &ts_casing())[0]);
        assert!(
            out.contains("export function validateCharge(value: Charge): ValidationError | null {")
        );
        assert!(out.contains("const violations: Violation[] = [];"));
        // An i64 is a bigint, so the bound carries the `n` suffix.
        assert!(out.contains("if (value.amount < 0n) {"));
        assert!(out.contains(
            "violations.push({ field: \"amount\", constraint: \"range\", message: \"amount must be >= 0\" });"
        ));
        // A string length counts code points via spread.
        assert!(out.contains("if ([...value.currency].length < 3) {"));
        assert!(out.contains("if ([...value.currency].length > 3) {"));
        // A failed validation is the taxonomy's Validation category, not a bare list.
        assert!(
            out.contains("return violations.length > 0 ? new ValidationError(violations) : null;")
        );
    }

    #[test]
    fn a_struct_without_constraints_emits_no_validator() {
        let shape = structure(
            "billing#note",
            vec![member("text", Tref::Prim(crate::ir::Prim::String), true)],
        );
        assert!(emit_validators(&shape, &ts_casing()).is_empty());
    }
    use crate::ir::{Prim, ShapeKind};

    #[test]
    fn a_structure_becomes_an_interface_with_cased_fields() {
        let shape = structure(
            "billing#Charge",
            vec![
                member("amount_cents", Tref::Prim(Prim::I64), true),
                member("note", Tref::Prim(Prim::String), false),
            ],
        );
        let decls = emit_type(&shape, &ts_casing());
        assert!(matches!(&decls[..], [Decl::Interface(i)]
            if i.name.name == "Charge"
                && i.fields[0].name.name == "amountCents"
                && !i.fields[0].nullable
                && i.fields[1].name.name == "note"
                && i.fields[1].nullable));
    }

    #[test]
    fn an_enum_becomes_a_literal_union_of_verbatim_tags() {
        let shape = enum_shape(
            "billing#Status",
            vec![("pending".into(), None), ("settled".into(), None)],
        );
        let decls = emit_type(&shape, &ts_casing());
        assert!(matches!(&decls[..], [Decl::Enum(d)]
            if d.name.name == "Status"
                && d.members.len() == 2
                && d.members[0].name == "pending"
                && d.members[1].name == "settled"
                && d.backing == EnumRepr::String));
    }

    #[test]
    fn an_int_backed_enum_becomes_a_numeric_literal_union() {
        let shape = int_enum_shape(
            "billing#http_code",
            vec![("ok".into(), Some(200)), ("error".into(), Some(500))],
        );
        let decls = emit_type(&shape, &ts_casing());
        assert!(matches!(&decls[..], [Decl::Enum(d)]
            if d.name.name == "HTTPCode"
                && d.backing == EnumRepr::Int(vec![200, 500])));
    }

    #[test]
    fn a_union_becomes_a_discriminated_union_decl() {
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
        let decls = emit_type(&shape, &ts_casing());
        assert!(matches!(&decls[..], [Decl::Union(u)]
            if u.name.name == "PaymentMethod"
                && u.discriminator == "type"
                && u.variants.len() == 1
                && u.variants[0].name.name == "card"
                && u.variants[0].payload.is_some()));
    }

    #[test]
    fn unsupported_shape_kinds_emit_nothing() {
        let service = Shape {
            id: "billing#Api".into(),
            kind: ShapeKind::Service { operations: vec![] },
            traits: vec![],
        };
        assert!(emit_type(&service, &ts_casing()).is_empty());
    }
}
