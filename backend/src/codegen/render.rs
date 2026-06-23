//! The render pipeline: a file's component tree to formatted source text.
//!
//! The engine owns the pipeline and runs it in a fixed order so output is
//! byte-stable: collect the file's imports (deterministically), render the
//! import block and then each declaration through the target's render rules into
//! rough text, and run the official formatter as the single layout authority.
//! Nothing here formats by hand; the target supplies only the language tokens.

use crate::codegen::format::{Formatted, Formatter};
use crate::codegen::imports;
use crate::codegen::target::RenderRules;
use crate::codegen::tree::File;

/// Render a file to formatted source. Imports come first (collected and ordered
/// by the engine), then a blank line, then the declarations separated by blank
/// lines; the whole rough text is handed to `formatter`.
pub fn render_file(file: &File, rules: &dyn RenderRules, formatter: &Formatter) -> Formatted {
    let imports = imports::collect(file);
    let mut rough = String::new();
    for import in &imports {
        rough.push_str(&rules.render_import(import));
        rough.push('\n');
    }
    if !imports.is_empty() {
        rough.push('\n');
    }
    for (index, decl) in file.decls.iter().enumerate() {
        if index > 0 {
            rough.push('\n');
        }
        rough.push_str(&rules.render_decl(decl));
        rough.push('\n');
    }
    formatter.run(&rough)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codegen::symbol::{Import, Symbol};
    use crate::codegen::target::{Fragment, Target};
    use crate::codegen::tree::{
        Decl, EnumDecl, Field, FnBody, Function, Interface, Method, TypeExpr, UnionDecl, Variant,
    };
    use crate::ir::{Member, Prim, Shape, ShapeKind, Tref};
    use serde_json::{json, Value};

    // A minimal Rust-flavored render-rules implementation: enough surface syntax
    // to exercise the pipeline and stand in for a real target's render rules.
    struct RustRules;

    impl RustRules {
        fn render_type(&self, ty: &TypeExpr) -> String {
            match ty {
                TypeExpr::Ref(symbol) => symbol.name.clone(),
                TypeExpr::List(inner) => format!("Vec<{}>", self.render_type(inner)),
                TypeExpr::Map(key, value) => {
                    format!(
                        "HashMap<{}, {}>",
                        self.render_type(key),
                        self.render_type(value)
                    )
                }
                TypeExpr::Nullable(inner) => format!("Option<{}>", self.render_type(inner)),
                TypeExpr::Generic(symbol, args) => {
                    let rendered: Vec<String> = args.iter().map(|a| self.render_type(a)).collect();
                    format!("{}<{}>", symbol.name, rendered.join(", "))
                }
            }
        }

        fn render_field(&self, field: &Field) -> String {
            let mut out = String::new();
            if let Some(wire) = &field.wire {
                // The @wire key rides the serialization axis; it never changes
                // the in-code identifier (field.name), proving the two are
                // independent.
                out.push_str(&format!("    #[serde(rename = \"{wire}\")]\n"));
            }
            let ty = if field.nullable {
                format!("Option<{}>", self.render_type(&field.ty))
            } else {
                self.render_type(&field.ty)
            };
            out.push_str(&format!("    {}: {ty},\n", field.name.name));
            out
        }

        fn render_sig(&self, params: &[Field], ret: &Option<TypeExpr>) -> String {
            let params: Vec<String> = params
                .iter()
                .map(|p| format!("{}: {}", p.name.name, self.render_type(&p.ty)))
                .collect();
            let ret = ret
                .as_ref()
                .map(|r| format!(" -> {}", self.render_type(r)))
                .unwrap_or_default();
            format!("({}){ret}", params.join(", "))
        }
    }

    impl RenderRules for RustRules {
        fn render_import(&self, import: &Import) -> String {
            format!("use {}::{};", import.module, import.imported)
        }

        fn render_decl(&self, decl: &Decl) -> String {
            match decl {
                Decl::Interface(interface) => {
                    let fields: String = interface
                        .fields
                        .iter()
                        .map(|f| self.render_field(f))
                        .collect();
                    format!("pub struct {} {{\n{fields}}}", interface.name.name)
                }
                Decl::Enum(decl) => {
                    let members: String = decl
                        .members
                        .iter()
                        .map(|m| format!("    {},\n", m.name))
                        .collect();
                    format!("pub enum {} {{\n{members}}}", decl.name.name)
                }
                Decl::Union(decl) => {
                    let variants: String = decl
                        .variants
                        .iter()
                        .map(|v| format!("    {},\n", v.name.name))
                        .collect();
                    format!("pub enum {} {{\n{variants}}}", decl.name.name)
                }
                Decl::Method(method) => {
                    let sig = self.render_sig(&method.params, &method.ret);
                    format!("pub fn {}{sig} {{ runtime.execute() }}", method.name.name)
                }
                Decl::Function(function) => {
                    let sig = self.render_sig(&function.params, &function.ret);
                    let FnBody::Raw { text, .. } = &function.body;
                    format!("pub fn {}{sig} {{ {text} }}", function.name.name)
                }
            }
        }
    }

    // A minimal target: maps IR types to symbols and a structure shape to an
    // interface declaration. Stands in for a real backend.
    struct TestTarget;

    impl TestTarget {
        fn local_name(id: &str) -> String {
            id.rsplit('#').next().unwrap_or(id).to_string()
        }

        fn split_id(id: &str) -> (String, String) {
            match id.split_once('#') {
                Some((module, name)) => (module.to_string(), name.to_string()),
                None => (String::new(), id.to_string()),
            }
        }

        fn prim_name(p: &Prim) -> String {
            // Every Prim serializes to a bare lowercase string.
            serde_json::to_value(p)
                .ok()
                .and_then(|v| v.as_str().map(str::to_string))
                .unwrap_or_default()
        }

        fn type_expr_of(&self, t: &Tref) -> TypeExpr {
            match t {
                Tref::List(inner) => TypeExpr::list(self.type_expr_of(inner)),
                Tref::Map(key, value) => {
                    TypeExpr::map(self.type_expr_of(key), self.type_expr_of(value))
                }
                Tref::Ref { args, .. } if !args.is_empty() => {
                    let rendered = args.iter().map(|a| self.type_expr_of(a)).collect();
                    TypeExpr::Generic(self.symbol_of(t), rendered)
                }
                Tref::Prim(_) | Tref::Param(_) | Tref::Ref { .. } => {
                    TypeExpr::Ref(self.symbol_of(t))
                }
            }
        }

        fn wire_of(traits: &[crate::ir::Trait]) -> Option<String> {
            traits
                .iter()
                .find(|t| t.id == "core#wire")
                .and_then(|t| t.value.as_str())
                .map(str::to_string)
        }
    }

    impl Target for TestTarget {
        fn name(&self) -> &str {
            "test"
        }

        fn symbol_of(&self, t: &Tref) -> Symbol {
            match t {
                Tref::Prim(p) => Symbol::builtin(Self::prim_name(p)),
                Tref::Param(name) => Symbol::builtin(name.clone()),
                Tref::Ref { id, .. } => {
                    let (module, name) = Self::split_id(id);
                    Symbol::imported(name.clone(), module, name)
                }
                Tref::List(_) => Symbol::builtin("Vec"),
                Tref::Map(_, _) => Symbol::builtin("HashMap"),
            }
        }

        fn emit_type(&self, shape: &Shape) -> Fragment {
            match &shape.kind {
                ShapeKind::Structure { members, .. } => {
                    let fields = members
                        .iter()
                        .map(|m| Field {
                            name: Symbol::builtin(m.name.clone()),
                            ty: self.type_expr_of(&m.target),
                            nullable: !m.required,
                            wire: Self::wire_of(&m.traits),
                        })
                        .collect();
                    vec![Decl::Interface(Interface {
                        name: Symbol::builtin(Self::local_name(&shape.id)),
                        fields,
                    })]
                }
                _ => vec![],
            }
        }

        fn emit_op_stub(&self, op: &Shape, _descriptor: &Value) -> Fragment {
            // The descriptor is opaque and deliberately unused here: the target
            // never interprets it. A real backend embeds it as a blob.
            vec![Decl::Method(Method {
                name: Symbol::builtin(Self::local_name(&op.id)),
                params: vec![],
                ret: None,
            })]
        }

        fn runtime_pkg(&self) -> &str {
            "test-runtime"
        }
    }

    // A formatter that leaves text unchanged: it makes the pipeline output exact
    // and idempotent, so golden and determinism checks do not depend on an
    // external formatter being installed.
    fn passthrough() -> Formatter {
        Formatter::new("cat", vec![])
    }

    fn imported_field(name: &str, ty_name: &str, module: &str) -> Field {
        Field {
            name: Symbol::builtin(name),
            ty: TypeExpr::Ref(Symbol::imported(ty_name, module, ty_name)),
            nullable: false,
            wire: None,
        }
    }

    #[test]
    fn render_assembles_sorted_imports_then_declarations() {
        let file = File {
            module: "billing".into(),
            decls: vec![Decl::Interface(Interface {
                name: Symbol::builtin("Invoice"),
                fields: vec![
                    imported_field("a", "A", "alpha"),
                    imported_field("z", "Z", "zeta"),
                ],
            })],
        };
        let out = render_file(&file, &RustRules, &passthrough());
        assert_eq!(out.warning, None);
        assert_eq!(
            out.text,
            "use alpha::A;\nuse zeta::Z;\n\npub struct Invoice {\n    a: A,\n    z: Z,\n}\n"
        );
    }

    #[test]
    fn render_emits_a_function_and_collects_its_body_refs() {
        // A codec-style function: its return type and the symbols its (opaque)
        // body references all feed import collection.
        let file = File {
            module: "billing".into(),
            decls: vec![Decl::Function(Function {
                name: Symbol::builtin("decodeCharge"),
                params: vec![Field {
                    name: Symbol::builtin("raw"),
                    ty: TypeExpr::Ref(Symbol::builtin("unknown")),
                    nullable: false,
                    wire: None,
                }],
                ret: Some(TypeExpr::Ref(Symbol::imported("Charge", "model", "Charge"))),
                body: FnBody::Raw {
                    text: "return decodeUuid(raw.id);".into(),
                    refs: vec![Symbol::imported("decodeUuid", "codecs", "decodeUuid")],
                },
            })],
        };
        let out = render_file(&file, &RustRules, &passthrough()).text;
        assert!(out.contains(
            "pub fn decodeCharge(raw: unknown) -> Charge { return decodeUuid(raw.id); }"
        ));
        // The return type and a body-referenced symbol are both imported.
        assert!(out.contains("use model::Charge;"));
        assert!(out.contains("use codecs::decodeUuid;"));
    }

    #[test]
    fn a_file_with_only_builtins_has_no_import_block() {
        let file = File {
            module: "billing".into(),
            decls: vec![Decl::Interface(Interface {
                name: Symbol::builtin("Plain"),
                fields: vec![Field {
                    name: Symbol::builtin("id"),
                    ty: TypeExpr::Ref(Symbol::builtin("String")),
                    nullable: false,
                    wire: None,
                }],
            })],
        };
        let out = render_file(&file, &RustRules, &passthrough());
        assert_eq!(out.text, "pub struct Plain {\n    id: String,\n}\n");
    }

    #[test]
    fn rename_and_wire_are_independent_axes() {
        // The identifier comes from @rename (already on the name symbol); the
        // wire key comes from @wire. They differ and coexist.
        let file = File {
            module: "billing".into(),
            decls: vec![Decl::Interface(Interface {
                name: Symbol::builtin("Charge"),
                fields: vec![Field {
                    name: Symbol::builtin("amountCents"),
                    ty: TypeExpr::Ref(Symbol::builtin("i64")),
                    nullable: false,
                    wire: Some("amount_cents".into()),
                }],
            })],
        };
        let out = render_file(&file, &RustRules, &passthrough());
        assert!(out.text.contains("#[serde(rename = \"amount_cents\")]"));
        assert!(out.text.contains("amountCents: i64,"));
        // The in-code identifier is not the wire key.
        assert!(!out.text.contains("amount_cents:"));
    }

    #[test]
    fn rendering_is_deterministic_and_idempotent() {
        let file = File {
            module: "billing".into(),
            decls: vec![Decl::Interface(Interface {
                name: Symbol::builtin("Invoice"),
                fields: vec![imported_field("a", "A", "alpha")],
            })],
        };
        let first = render_file(&file, &RustRules, &passthrough());
        let second = render_file(&file, &RustRules, &passthrough());
        assert_eq!(
            first.text, second.text,
            "same input must produce same output"
        );
        // Re-running the formatter on the output is a no-op.
        let reformatted = passthrough().run(&first.text);
        assert_eq!(reformatted.text, first.text);
    }

    #[test]
    fn render_covers_every_type_form_and_declaration_kind() {
        let file = File {
            module: "billing".into(),
            decls: vec![
                Decl::Interface(Interface {
                    name: Symbol::builtin("Shapes"),
                    fields: vec![
                        Field {
                            name: Symbol::builtin("items"),
                            ty: TypeExpr::list(TypeExpr::Ref(Symbol::imported(
                                "Item", "catalog", "Item",
                            ))),
                            nullable: false,
                            wire: None,
                        },
                        Field {
                            name: Symbol::builtin("index"),
                            ty: TypeExpr::map(
                                TypeExpr::Ref(Symbol::builtin("String")),
                                TypeExpr::Ref(Symbol::builtin("i64")),
                            ),
                            nullable: false,
                            wire: None,
                        },
                        Field {
                            name: Symbol::builtin("note"),
                            ty: TypeExpr::nullable(TypeExpr::Ref(Symbol::builtin("String"))),
                            nullable: false,
                            wire: None,
                        },
                        Field {
                            name: Symbol::builtin("page"),
                            ty: TypeExpr::Generic(
                                Symbol::imported("Page", "core", "Page"),
                                vec![TypeExpr::Ref(Symbol::imported("Item", "catalog", "Item"))],
                            ),
                            nullable: true,
                            wire: None,
                        },
                    ],
                }),
                Decl::Enum(EnumDecl {
                    name: Symbol::builtin("Status"),
                    members: vec![Symbol::builtin("Active"), Symbol::builtin("Closed")],
                }),
                Decl::Union(UnionDecl {
                    name: Symbol::builtin("Method"),
                    discriminator: "type".into(),
                    variants: vec![Variant {
                        name: Symbol::builtin("Card"),
                        fields: vec![],
                        wire: None,
                    }],
                }),
                Decl::Method(Method {
                    name: Symbol::builtin("create"),
                    params: vec![Field {
                        name: Symbol::builtin("input"),
                        ty: TypeExpr::Ref(Symbol::builtin("String")),
                        nullable: false,
                        wire: None,
                    }],
                    ret: Some(TypeExpr::Ref(Symbol::builtin("String"))),
                }),
            ],
        };
        let out = render_file(&file, &RustRules, &passthrough()).text;
        assert!(out.contains("items: Vec<Item>,"));
        assert!(out.contains("index: HashMap<String, i64>,"));
        assert!(out.contains("note: Option<String>,"));
        assert!(out.contains("page: Option<Page<Item>>,"));
        assert!(out.contains("pub enum Status {"));
        assert!(out.contains("pub enum Method {"));
        assert!(out.contains("pub fn create(input: String) -> String { runtime.execute() }"));
    }

    #[test]
    fn target_maps_a_structure_shape_end_to_end() {
        let shape = Shape {
            id: "billing#Invoice".into(),
            kind: ShapeKind::Structure {
                params: vec![],
                members: vec![
                    Member {
                        name: "amount".into(),
                        target: Tref::Prim(Prim::I64),
                        required: true,
                        default: None,
                        constraints: vec![],
                        traits: vec![crate::ir::Trait {
                            id: "core#wire".into(),
                            value: json!("amount_cents"),
                        }],
                    },
                    Member {
                        name: "customer".into(),
                        target: Tref::Ref {
                            id: "crm#Customer".into(),
                            args: vec![],
                        },
                        required: false,
                        default: None,
                        constraints: vec![],
                        traits: vec![],
                    },
                    Member {
                        name: "tags".into(),
                        target: Tref::List(Box::new(Tref::Prim(Prim::String))),
                        required: true,
                        default: None,
                        constraints: vec![],
                        traits: vec![],
                    },
                    Member {
                        name: "meta".into(),
                        target: Tref::Map(
                            Box::new(Tref::Prim(Prim::String)),
                            Box::new(Tref::Prim(Prim::String)),
                        ),
                        required: true,
                        default: None,
                        constraints: vec![],
                        traits: vec![],
                    },
                    Member {
                        name: "page".into(),
                        target: Tref::Ref {
                            id: "core#Page".into(),
                            args: vec![Tref::Ref {
                                id: "crm#Customer".into(),
                                args: vec![],
                            }],
                        },
                        required: true,
                        default: None,
                        constraints: vec![],
                        traits: vec![],
                    },
                ],
            },
            traits: vec![],
        };
        let target = TestTarget;
        let decls = target.emit_type(&shape);
        let file = File {
            module: "billing".into(),
            decls,
        };
        let out = render_file(&file, &RustRules, &passthrough()).text;
        // Cross-module type pulls its import; the @wire trait becomes a rename;
        // the optional member becomes Option.
        assert!(out.contains("use crm::Customer;"));
        assert!(out.contains("pub struct Invoice {"));
        assert!(out.contains("#[serde(rename = \"amount_cents\")]"));
        assert!(out.contains("amount: i64,"));
        assert!(out.contains("customer: Option<Customer>,"));
        // Collections and a cross-module generic application map through too.
        assert!(out.contains("tags: Vec<string>,"));
        assert!(out.contains("meta: HashMap<string, string>,"));
        assert!(out.contains("page: Page<Customer>,"));
        assert!(out.contains("use core::Page;"));
    }

    #[test]
    fn target_surface_methods_are_usable() {
        let target = TestTarget;
        assert_eq!(target.name(), "test");
        assert_eq!(target.runtime_pkg(), "test-runtime");
        // symbol_of over every Tref form.
        assert_eq!(target.symbol_of(&Tref::Prim(Prim::String)).name, "string");
        assert_eq!(target.symbol_of(&Tref::Param("T".into())).name, "T");
        assert_eq!(
            target
                .symbol_of(&Tref::Ref {
                    id: "m#N".into(),
                    args: vec![],
                })
                .name,
            "N"
        );
        assert_eq!(
            target
                .symbol_of(&Tref::List(Box::new(Tref::Prim(Prim::Bool))))
                .name,
            "Vec"
        );
        assert_eq!(
            target
                .symbol_of(&Tref::Map(
                    Box::new(Tref::Prim(Prim::String)),
                    Box::new(Tref::Prim(Prim::Bool)),
                ))
                .name,
            "HashMap"
        );
        // A ref id without a module separator keeps the whole id as the name.
        assert_eq!(
            target
                .symbol_of(&Tref::Ref {
                    id: "Bare".into(),
                    args: vec![],
                })
                .name,
            "Bare"
        );
        // emit_op_stub embeds nothing it interprets; the descriptor is ignored.
        let op = Shape {
            id: "billing#CreateInvoice".into(),
            kind: ShapeKind::Operation {
                input: None,
                output: None,
                errors: vec![],
            },
            traits: vec![],
        };
        let stub = target.emit_op_stub(&op, &json!({"http_method": "POST"}));
        assert_eq!(stub.len(), 1);
        // A non-structure shape emits nothing from emit_type.
        assert!(target.emit_type(&op).is_empty());
    }
}
