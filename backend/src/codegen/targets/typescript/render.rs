//! The TypeScript render rules: how the shared component tree turns into TS
//! surface syntax. Imports, interfaces, and open-enum literal unions are
//! rendered here; unions, methods, and generated functions are added by later
//! phases.

use crate::codegen::syntax::{self, TypeSyntax};
use crate::codegen::target::RenderRules;
use crate::codegen::tree::{Decl, EnumRepr, Field, FnBody, Function, Method, TypeExpr, Variant};

/// The TypeScript render rules.
pub struct TsRules;

/// The TypeScript spelling of each composite type construct; the recursion lives
/// in the shared `syntax` driver. An `@entries` map is already the
/// `[[k, v], …]` wire shape (a `[K, V]` tuple list).
impl TypeSyntax for TsRules {
    fn list(&self, inner: &str) -> String {
        // A nullable element needs parentheses before `[]` binds.
        if inner.ends_with(" | null") {
            format!("({inner})[]")
        } else {
            format!("{inner}[]")
        }
    }
    fn map(&self, key: &str, value: &str) -> String {
        format!("Record<{key}, {value}>")
    }
    fn nullable(&self, inner: &str) -> String {
        format!("{inner} | null")
    }
    fn generic(&self, name: &str, args: &[String]) -> String {
        format!("{name}<{}>", args.join(", "))
    }
    fn entries(&self, key: &str, value: &str) -> String {
        format!("[{key}, {value}][]")
    }
}

impl TsRules {
    fn render_type(&self, ty: &TypeExpr) -> String {
        syntax::render_type(ty, self)
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

    /// One client method signature. An async operation returns a `Promise` (the
    /// caller awaits it); a sync one returns the plain type. Errors are thrown,
    /// so the method's error channel does not appear in the signature.
    fn render_method(&self, method: &Method) -> String {
        let params: Vec<String> = method
            .params
            .iter()
            .map(|p| format!("{}: {}", p.name.name, self.render_type(&p.ty)))
            .collect();
        let ret = method
            .ret
            .as_ref()
            .map(|r| self.render_type(r))
            .unwrap_or_else(|| "void".into());
        let ret = if method.is_async {
            format!("Promise<{ret}>")
        } else {
            ret
        };
        format!("  {}({}): {ret};\n", method.name.name, params.join(", "))
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
    fn render_import(&self, module: &str, names: &[&str]) -> String {
        format!("import {{ {} }} from \"./{module}\";", names.join(", "))
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
                // Open enum: known literals plus an open arm that keeps autocomplete
                // for the literals while still accepting any value of the backing
                // type on decode. String-backed members are quoted wire tags;
                // int-backed members are bare integer literals.
                let (mut arms, open): (Vec<String>, &str) = match &decl.backing {
                    EnumRepr::String => (
                        decl.members
                            .iter()
                            .map(|m| format!("\"{}\"", m.name))
                            .collect(),
                        "(string & {})",
                    ),
                    EnumRepr::Int(ints) => (
                        ints.iter().map(|n| n.to_string()).collect(),
                        "(number & {})",
                    ),
                };
                arms.push(open.into());
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
            Decl::Raw(raw) => raw.text.clone(),
            Decl::Client(client) => {
                let methods: String = client
                    .methods
                    .iter()
                    .map(|m| self.render_method(m))
                    .collect();
                format!("export interface {} {{\n{methods}}}", client.name.name)
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
    use crate::codegen::tree::{
        EnumDecl, EnumRepr, FnBody, Function, Interface, Method, Raw, UnionDecl, Variant,
    };

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
            TsRules.render_import("payments", &["Charge"]),
            "import { Charge } from \"./payments\";"
        );
        // Several names from one module group into one import statement.
        assert_eq!(
            TsRules.render_import("payments", &["BankAccount", "Card", "Charge"]),
            "import { BankAccount, Card, Charge } from \"./payments\";"
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
        assert_eq!(
            rules.render_type(&TypeExpr::entries(
                TypeExpr::Ref(Symbol::builtin("number")),
                TypeExpr::Ref(Symbol::builtin("Charge")),
            )),
            "[number, Charge][]"
        );
    }

    #[test]
    fn an_open_enum_renders_literals_plus_open_arm() {
        let decl = Decl::Enum(EnumDecl {
            name: Symbol::builtin("Status"),
            members: vec![Symbol::builtin("pending"), Symbol::builtin("settled")],
            backing: EnumRepr::String,
        });
        assert_eq!(
            TsRules.render_decl(&decl),
            "export type Status = \"pending\" | \"settled\" | (string & {});"
        );
    }

    #[test]
    fn an_int_backed_enum_renders_a_numeric_literal_union_with_an_open_number_arm() {
        let decl = Decl::Enum(EnumDecl {
            name: Symbol::builtin("HTTPCode"),
            members: vec![
                Symbol::builtin("ok"),
                Symbol::builtin("not_found"),
                Symbol::builtin("error"),
            ],
            backing: EnumRepr::Int(vec![200, 404, 500]),
        });
        assert_eq!(
            TsRules.render_decl(&decl),
            "export type HTTPCode = 200 | 404 | 500 | (number & {});"
        );
    }

    #[test]
    fn an_empty_enum_is_just_the_open_arm() {
        let decl = Decl::Enum(EnumDecl {
            name: Symbol::builtin("Empty"),
            members: vec![],
            backing: EnumRepr::String,
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
    fn a_raw_decl_renders_its_text_verbatim() {
        let raw = Decl::Raw(Raw {
            text: "export const VERSION = \"1\";".into(),
            refs: vec![],
        });
        assert_eq!(TsRules.render_decl(&raw), "export const VERSION = \"1\";");
    }

    #[test]
    fn operation_methods_render_nothing_yet() {
        let method = Decl::Method(Method {
            name: Symbol::builtin("ping"),
            params: vec![],
            ret: None,
            err: None,
            is_async: false,
        });
        assert_eq!(TsRules.render_decl(&method), "");
    }

    #[test]
    fn a_client_renders_method_signatures_with_the_effect_lowered() {
        let decl = Decl::Client(crate::codegen::tree::ClientDecl {
            name: Symbol::builtin("Client"),
            methods: vec![
                Method {
                    name: Symbol::builtin("createCharge"),
                    params: vec![field(
                        "input",
                        TypeExpr::Ref(Symbol::builtin("CreateChargeInput")),
                        false,
                    )],
                    ret: Some(TypeExpr::Ref(Symbol::builtin("Charge"))),
                    // Errors are thrown in TS, so the channel stays out of the signature.
                    err: Some(TypeExpr::Ref(Symbol::builtin("TonoError"))),
                    is_async: true,
                },
                Method {
                    name: Symbol::builtin("localOp"),
                    params: vec![],
                    ret: None,
                    err: None,
                    is_async: false,
                },
            ],
        });
        assert_eq!(
            TsRules.render_decl(&decl),
            "export interface Client {\n  createCharge(input: CreateChargeInput): \
             Promise<Charge>;\n  localOp(): void;\n}"
        );
    }
}
