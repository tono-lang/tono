//! The TypeScript render rules: how the shared component tree turns into TS
//! surface syntax. Imports, interfaces, and open-enum literal unions are
//! rendered here; unions, methods, and generated functions are added by later
//! phases.

use crate::codegen::symbol::Import;
use crate::codegen::target::RenderRules;
use crate::codegen::tree::{Decl, Field, FnBody, Function, TypeExpr, Variant};

/// The TypeScript render rules.
pub struct TsRules;

impl TsRules {
    fn render_type(&self, ty: &TypeExpr) -> String {
        match ty {
            TypeExpr::Ref(symbol) => symbol.name.clone(),
            TypeExpr::List(inner) => {
                let rendered = self.render_type(inner);
                // A union element needs parentheses before `[]` binds.
                if matches!(**inner, TypeExpr::Nullable(_)) {
                    format!("({rendered})[]")
                } else {
                    format!("{rendered}[]")
                }
            }
            TypeExpr::Map(key, value) => {
                format!(
                    "Record<{}, {}>",
                    self.render_type(key),
                    self.render_type(value)
                )
            }
            TypeExpr::Nullable(inner) => format!("{} | null", self.render_type(inner)),
            TypeExpr::Generic(symbol, args) => {
                let rendered: Vec<String> = args.iter().map(|a| self.render_type(a)).collect();
                format!("{}<{}>", symbol.name, rendered.join(", "))
            }
        }
    }

    fn render_field(&self, field: &Field) -> String {
        let ty = self.render_type(&field.ty);
        // Nullable maps to an optional field that also admits an explicit null.
        if field.nullable {
            format!("  {}?: {ty} | null;\n", field.name.name)
        } else {
            format!("  {}: {ty};\n", field.name.name)
        }
    }

    fn render_function(&self, function: &Function) -> String {
        let params: Vec<String> = function
            .params
            .iter()
            .map(|p| format!("{}: {}", p.name.name, self.render_type(&p.ty)))
            .collect();
        let ret = function
            .ret
            .as_ref()
            .map(|r| format!(": {}", self.render_type(r)))
            .unwrap_or_default();
        let FnBody::Raw { text, .. } = &function.body;
        format!(
            "export function {}({}){ret} {{\n{text}\n}}",
            function.name.name,
            params.join(", ")
        )
    }

    fn render_variant(&self, discriminator: &str, variant: &Variant) -> String {
        let tag = variant
            .wire
            .as_deref()
            .unwrap_or(variant.name.name.as_str());
        let head = format!("{{ {discriminator}: \"{tag}\" }}");
        // A variant with a payload intersects the discriminator object with it;
        // a payload-less variant is a bare tag (a marker variant).
        match &variant.payload {
            Some(payload) => format!("({head} & {})", self.render_type(payload)),
            None => head,
        }
    }
}

impl RenderRules for TsRules {
    fn render_import(&self, import: &Import) -> String {
        format!(
            "import {{ {} }} from \"./{}\";",
            import.imported, import.module
        )
    }

    fn render_decl(&self, decl: &Decl) -> String {
        match decl {
            Decl::Interface(interface) => {
                let fields: String = interface
                    .fields
                    .iter()
                    .map(|f| self.render_field(f))
                    .collect();
                format!("export interface {} {{\n{fields}}}", interface.name.name)
            }
            Decl::Enum(decl) => {
                // Open enum: known literals plus `(string & {})`, which keeps
                // autocomplete for the literals while still accepting any string
                // on decode.
                let mut arms: Vec<String> = decl
                    .members
                    .iter()
                    .map(|m| format!("\"{}\"", m.name))
                    .collect();
                arms.push("(string & {})".into());
                format!("export type {} = {};", decl.name.name, arms.join(" | "))
            }
            Decl::Union(decl) => {
                let arms: Vec<String> = decl
                    .variants
                    .iter()
                    .map(|v| self.render_variant(&decl.discriminator, v))
                    .collect();
                format!("export type {} = {};", decl.name.name, arms.join(" | "))
            }
            Decl::Function(function) => self.render_function(function),
            Decl::Alias(alias) => {
                format!("export type {} = {};", alias.name.name, alias.value)
            }
            // Operation-stub methods are emitted by a later phase.
            Decl::Method(_) => String::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codegen::symbol::Symbol;
    use crate::codegen::tree::{EnumDecl, FnBody, Function, Interface, Method, UnionDecl, Variant};

    fn field(name: &str, ty: TypeExpr, nullable: bool) -> Field {
        Field {
            name: Symbol::builtin(name),
            ty,
            nullable,
            wire: None,
        }
    }

    #[test]
    fn imports_render_as_named_imports() {
        assert_eq!(
            TsRules.render_import(&Import {
                module: "payments".into(),
                imported: "Charge".into(),
            }),
            "import { Charge } from \"./payments\";"
        );
    }

    #[test]
    fn an_interface_renders_fields_with_nullability() {
        let decl = Decl::Interface(Interface {
            name: Symbol::builtin("Charge"),
            fields: vec![
                field("id", TypeExpr::Ref(Symbol::builtin("string")), false),
                field("note", TypeExpr::Ref(Symbol::builtin("string")), true),
            ],
        });
        assert_eq!(
            TsRules.render_decl(&decl),
            "export interface Charge {\n  id: string;\n  note?: string | null;\n}"
        );
    }

    #[test]
    fn type_expressions_render_idiomatically() {
        let rules = TsRules;
        assert_eq!(
            rules.render_type(&TypeExpr::list(TypeExpr::Ref(Symbol::builtin("Charge")))),
            "Charge[]"
        );
        assert_eq!(
            rules.render_type(&TypeExpr::list(TypeExpr::nullable(TypeExpr::Ref(
                Symbol::builtin("Charge")
            )))),
            "(Charge | null)[]"
        );
        assert_eq!(
            rules.render_type(&TypeExpr::map(
                TypeExpr::Ref(Symbol::builtin("string")),
                TypeExpr::Ref(Symbol::builtin("Charge")),
            )),
            "Record<string, Charge>"
        );
        assert_eq!(
            rules.render_type(&TypeExpr::nullable(TypeExpr::Ref(Symbol::builtin(
                "Charge"
            )))),
            "Charge | null"
        );
        assert_eq!(
            rules.render_type(&TypeExpr::Generic(
                Symbol::builtin("Page"),
                vec![TypeExpr::Ref(Symbol::builtin("Charge"))],
            )),
            "Page<Charge>"
        );
    }

    #[test]
    fn an_open_enum_renders_literals_plus_open_arm() {
        let decl = Decl::Enum(EnumDecl {
            name: Symbol::builtin("Status"),
            members: vec![Symbol::builtin("pending"), Symbol::builtin("settled")],
        });
        assert_eq!(
            TsRules.render_decl(&decl),
            "export type Status = \"pending\" | \"settled\" | (string & {});"
        );
    }

    #[test]
    fn an_empty_enum_is_just_the_open_arm() {
        let decl = Decl::Enum(EnumDecl {
            name: Symbol::builtin("Empty"),
            members: vec![],
        });
        assert_eq!(
            TsRules.render_decl(&decl),
            "export type Empty = (string & {});"
        );
    }

    #[test]
    fn a_union_renders_as_a_discriminated_union() {
        // A variant with a payload intersects the discriminator object with it;
        // a payload-less variant is a bare tag, and its tag honors @wire.
        let decl = Decl::Union(UnionDecl {
            name: Symbol::builtin("PaymentMethod"),
            discriminator: "kind".into(),
            variants: vec![
                Variant {
                    name: Symbol::builtin("card"),
                    fields: vec![],
                    payload: Some(TypeExpr::Ref(Symbol::builtin("CardData"))),
                    wire: None,
                },
                Variant {
                    name: Symbol::builtin("cash"),
                    fields: vec![],
                    payload: None,
                    wire: Some("CASH".into()),
                },
            ],
        });
        assert_eq!(
            TsRules.render_decl(&decl),
            "export type PaymentMethod = ({ kind: \"card\" } & CardData) | { kind: \"CASH\" };"
        );
    }

    #[test]
    fn a_function_renders_with_its_signature_and_body() {
        let function = Decl::Function(Function {
            name: Symbol::builtin("decodeI64"),
            params: vec![field("s", TypeExpr::Ref(Symbol::builtin("string")), false)],
            ret: Some(TypeExpr::Ref(Symbol::builtin("bigint"))),
            body: FnBody::Raw {
                text: "  return BigInt(s);".into(),
                refs: vec![],
            },
        });
        assert_eq!(
            TsRules.render_decl(&function),
            "export function decodeI64(s: string): bigint {\n  return BigInt(s);\n}"
        );
    }

    #[test]
    fn operation_methods_render_nothing_yet() {
        let method = Decl::Method(Method {
            name: Symbol::builtin("ping"),
            params: vec![],
            ret: None,
        });
        assert_eq!(TsRules.render_decl(&method), "");
    }
}
