//! The TypeScript render rules: how the shared component tree turns into TS
//! surface syntax. Imports, interfaces, and open-enum literal unions are
//! rendered here; unions, methods, and generated functions are added by later
//! phases.

use crate::codegen::doc;
use crate::codegen::syntax::{self, TypeSyntax};
use crate::codegen::target::RenderRules;
use crate::codegen::tree::{Decl, EnumRepr, Field, FnBody, Function, Method, TypeExpr, Variant};

/// The TypeScript render rules.
pub struct TsRules;

/// The generic type-parameter clause of a definition (`<T>`, `<T, U>`), or the
/// empty string for a non-generic shape. TypeScript needs no parameter bound, so
/// each name renders bare.
fn type_params(params: &[String]) -> String {
    if params.is_empty() {
        String::new()
    } else {
        format!("<{}>", params.join(", "))
    }
}

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
        let dep = doc::jsdoc_block(field.doc.as_deref(), field.deprecated.as_deref(), "  ");
        if field.nullable {
            format!("{dep}  {}?: {ty} | null;\n", field.name.name)
        } else {
            format!("{dep}  {}: {ty};\n", field.name.name)
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
        let jsdoc = doc::jsdoc_block(method.doc.as_deref(), None, "  ");
        format!(
            "{jsdoc}  {}({}): {ret};\n",
            method.name.name,
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
    fn render_import(&self, from_module: &str, module: &str, names: &[&str]) -> String {
        // One of the SDK's own groups is a file, so it is imported by a path
        // relative to the importing file; a bare package specifier (the
        // hand-written runtime, a scoped `@scope/name`) is not a group and is
        // imported as-is.
        let path = crate::codegen::layout::ts_specifier(from_module, module)
            .unwrap_or_else(|| module.to_string());
        format!("import {{ {} }} from \"{path}\";", names.join(", "))
    }

    fn render_decl(&self, decl: &Decl) -> String {
        match decl {
            Decl::Interface(interface) => {
                let fields: String = interface
                    .fields
                    .iter()
                    .map(|f| self.render_field(f))
                    .collect();
                let dep = doc::jsdoc_block(
                    interface.doc.as_deref(),
                    interface.deprecated.as_deref(),
                    "",
                );
                let params = type_params(&interface.params);
                format!(
                    "{dep}export interface {}{params} {{\n{fields}}}",
                    interface.name.name
                )
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
                // A literal-union type alias has no per-member slot, so member docs
                // are not expressible in TypeScript; only the enum-level doc is.
                let dep = doc::jsdoc_block(decl.doc.as_deref(), decl.deprecated.as_deref(), "");
                format!(
                    "{dep}export type {} = {};",
                    decl.name.name,
                    arms.join(" | ")
                )
            }
            Decl::Union(decl) => {
                let arms: Vec<String> = decl
                    .variants
                    .iter()
                    .map(|v| self.render_variant(&decl.discriminator, v))
                    .collect();
                let dep = doc::jsdoc_block(decl.doc.as_deref(), decl.deprecated.as_deref(), "");
                format!(
                    "{dep}export type {} = {};",
                    decl.name.name,
                    arms.join(" | ")
                )
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
            tag: None,
            name: Symbol::builtin(name),
            ty,
            nullable,
            wire: None,
            deprecated: None,
            doc: None,
        }
    }

    #[test]
    fn imports_render_as_named_imports() {
        assert_eq!(
            TsRules.render_import("billing::types", "payments::types", &["Charge"]),
            "import { Charge } from \"../payments/types\";"
        );
        // Several names from one module group into one import statement.
        assert_eq!(
            TsRules.render_import(
                "billing::types",
                "payments::types",
                &["BankAccount", "Card", "Charge"],
            ),
            "import { BankAccount, Card, Charge } from \"../payments/types\";"
        );
    }

    #[test]
    fn an_external_package_imports_as_a_bare_specifier() {
        // The hand-written runtime is a scoped package, not a sibling module, so
        // it keeps its bare specifier rather than gaining a `./` prefix.
        assert_eq!(
            TsRules.render_import(
                "payments.charges",
                "@tono/http-runtime-ts",
                &["execute", "WireDescriptor"]
            ),
            "import { execute, WireDescriptor } from \"@tono/http-runtime-ts\";"
        );
    }

    #[test]
    fn a_group_is_imported_by_a_path_relative_to_the_importer() {
        // A sibling group of the same module is `./name`; another module walks
        // up out of its directory and back down.
        assert_eq!(
            TsRules.render_import(
                "payments.charges::types",
                "payments.charges::codec",
                &["encodeCharge"]
            ),
            "import { encodeCharge } from \"./codec\";"
        );
        assert_eq!(
            TsRules.render_import(
                "payments.charges::types",
                "payments.common::types",
                &["Money"]
            ),
            "import { Money } from \"../common/types\";"
        );
        // Anything that is not one of the SDK's groups (the hand-written
        // runtime) is imported by its bare specifier.
        assert_eq!(
            TsRules.render_import(
                "payments.charges::types",
                "@tono/http-runtime-ts",
                &["execute"]
            ),
            "import { execute } from \"@tono/http-runtime-ts\";"
        );
    }

    #[test]
    fn doc_renders_as_jsdoc_and_combines_with_deprecation() {
        let decl = Decl::Interface(Interface {
            name: Symbol::builtin("Charge"),
            params: vec![],
            fields: vec![Field {
                tag: None,
                name: Symbol::builtin("amount"),
                ty: TypeExpr::Ref(Symbol::builtin("number")),
                nullable: false,
                wire: None,
                deprecated: Some("use amountCents".into()),
                doc: Some("The amount.".into()),
            }],
            deprecated: None,
            doc: Some("A billing charge.\n\nMarkdown rides JSDoc verbatim.".into()),
        });
        let out = TsRules.render_decl(&decl);
        // A multi-line doc becomes a JSDoc block above the interface.
        assert!(out.starts_with(
            "/**\n * A billing charge.\n *\n * Markdown rides JSDoc verbatim.\n */\nexport interface Charge"
        ));
        // The field combines its doc and the @deprecated tag into one block.
        assert!(out.contains(
            "  /**\n   * The amount.\n   * @deprecated use amountCents\n   */\n  amount: number;"
        ));
    }

    #[test]
    fn an_interface_renders_fields_with_nullability() {
        let decl = Decl::Interface(Interface {
            name: Symbol::builtin("Charge"),
            params: vec![],
            fields: vec![
                field("id", TypeExpr::Ref(Symbol::builtin("string")), false),
                field("note", TypeExpr::Ref(Symbol::builtin("string")), true),
            ],
            deprecated: None,
            doc: None,
        });
        assert_eq!(
            TsRules.render_decl(&decl),
            "export interface Charge {\n  id: string;\n  note?: string | null;\n}"
        );
    }

    #[test]
    fn deprecated_decls_and_fields_carry_jsdoc() {
        let iface = Decl::Interface(Interface {
            name: Symbol::builtin("Charge"),
            params: vec![],
            fields: vec![Field {
                tag: None,
                name: Symbol::builtin("amount"),
                ty: TypeExpr::Ref(Symbol::builtin("number")),
                nullable: false,
                wire: None,
                deprecated: Some("use amountCents".into()),
                doc: None,
            }],
            deprecated: Some("use ChargeV2".into()),
            doc: None,
        });
        let out = TsRules.render_decl(&iface);
        assert!(out.starts_with("/** @deprecated use ChargeV2 */\nexport interface Charge"));
        assert!(out.contains("  /** @deprecated use amountCents */\n  amount: number;"));

        // A bare `@deprecated` renders the tag with no reason, on an enum and a union.
        let enum_decl = Decl::Enum(EnumDecl {
            name: Symbol::builtin("Status"),
            members: vec![Symbol::builtin("open")],
            member_docs: vec![None],
            backing: EnumRepr::String,
            deprecated: Some(String::new()),
            doc: None,
        });
        assert!(TsRules
            .render_decl(&enum_decl)
            .starts_with("/** @deprecated */\nexport type Status"));

        let union = Decl::Union(UnionDecl {
            name: Symbol::builtin("Source"),
            discriminator: "type".into(),
            variants: vec![],
            deprecated: Some("gone".into()),
            doc: None,
        });
        assert!(TsRules
            .render_decl(&union)
            .starts_with("/** @deprecated gone */\nexport type Source"));
    }

    #[test]
    fn a_generic_interface_renders_its_type_parameter_clause() {
        let decl = Decl::Interface(Interface {
            name: Symbol::builtin("Page"),
            params: vec!["T".into()],
            fields: vec![field(
                "items",
                TypeExpr::list(TypeExpr::Ref(Symbol::builtin("T"))),
                false,
            )],
            deprecated: None,
            doc: None,
        });
        assert_eq!(
            TsRules.render_decl(&decl),
            "export interface Page<T> {\n  items: T[];\n}"
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
            member_docs: vec![None, None],
            backing: EnumRepr::String,
            deprecated: None,
            doc: None,
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
            member_docs: vec![None, None, None],
            backing: EnumRepr::Int(vec![200, 404, 500]),
            deprecated: None,
            doc: None,
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
            member_docs: vec![],
            backing: EnumRepr::String,
            deprecated: None,
            doc: None,
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
                    doc: None,
                },
                Variant {
                    name: Symbol::builtin("cash"),
                    fields: vec![],
                    payload: None,
                    wire: Some("CASH".into()),
                    doc: None,
                },
            ],
            deprecated: None,
            doc: None,
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
            ..Raw::default()
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
            doc: None,
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
                    doc: None,
                },
                Method {
                    name: Symbol::builtin("localOp"),
                    params: vec![],
                    ret: None,
                    err: None,
                    is_async: false,
                    doc: None,
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
