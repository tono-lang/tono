//! The Rust render rules: how the shared component tree turns into Rust surface
//! syntax. Structs carry serde derives and per-field attributes; the wire key
//! rides `#[serde(rename)]` and an optional field becomes `Option<T>` that is
//! skipped on serialize and defaulted on deserialize.
//!
//! Enums and unions are not rendered from `Decl::Enum`/`Decl::Union`: the open
//! enum needs a hand-written `Deserialize` (for its catch-all `Unknown` arm) and
//! a tagged union needs custom plumbing, so the Rust target emits both as
//! verbatim `Decl::Raw` items in a later phase. Their arms here render nothing.

use crate::codegen::syntax::{self, TypeSyntax};
use crate::codegen::target::RenderRules;
use crate::codegen::targets::rust::codecs::serde_with;
use crate::codegen::tree::{Decl, Field, FnBody, Function, TypeExpr};

/// The standard derives every generated struct and enum carries.
const DERIVES: &str = "#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]";

/// Render a component-tree type expression into Rust surface syntax. Free so the
/// codec layer (which builds union payload types) can reuse it.
pub(crate) fn type_string(ty: &TypeExpr) -> String {
    syntax::render_type(ty, &RustRules)
}

/// The Rust render rules.
pub struct RustRules;

/// The Rust spelling of each composite type construct; the recursion lives in the
/// shared `syntax` driver. `@entries` is a `Vec<(K, V)>`, which serde renders
/// directly as the `[[k, v], …]` wire array.
impl TypeSyntax for RustRules {
    fn list(&self, inner: &str) -> String {
        format!("Vec<{inner}>")
    }
    fn map(&self, key: &str, value: &str) -> String {
        format!("std::collections::HashMap<{key}, {value}>")
    }
    fn nullable(&self, inner: &str) -> String {
        format!("Option<{inner}>")
    }
    fn generic(&self, name: &str, args: &[String]) -> String {
        format!("{name}<{}>", args.join(", "))
    }
    fn entries(&self, key: &str, value: &str) -> String {
        format!("Vec<({key}, {value})>")
    }
}

impl RustRules {
    fn render_type(&self, ty: &TypeExpr) -> String {
        syntax::render_type(ty, self)
    }

    fn render_field(&self, field: &Field) -> String {
        let ty = if field.nullable {
            format!("Option<{}>", self.render_type(&field.ty))
        } else {
            self.render_type(&field.ty)
        };
        // The wire key rides the serialization axis (#[serde(rename)]); it never
        // changes the in-code identifier. An optional field is skipped when None
        // on serialize and defaulted when absent on deserialize. A 64-bit integer
        // or bytes field additionally routes through a custom `with` codec.
        let mut args: Vec<String> = Vec::new();
        if let Some(wire) = &field.wire {
            args.push(format!("rename = \"{wire}\""));
        }
        if field.nullable {
            args.push("default".into());
            args.push("skip_serializing_if = \"Option::is_none\"".into());
        }
        if let Some(with) = serde_with(field) {
            args.push(format!("with = \"{with}\""));
        }
        let attr = if args.is_empty() {
            String::new()
        } else {
            format!("    #[serde({})]\n", args.join(", "))
        };
        format!("{attr}    pub {}: {ty},\n", field.name.name)
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
            .map(|r| format!(" -> {}", self.render_type(r)))
            .unwrap_or_default();
        let FnBody::Raw { text, .. } = &function.body;
        format!(
            "pub fn {}({}){ret} {{\n{text}\n}}",
            function.name.name,
            params.join(", ")
        )
    }
}

impl RenderRules for RustRules {
    fn render_import(&self, module: &str, names: &[&str]) -> String {
        // A single name needs no braces; several group into one `use`.
        if let [name] = names {
            format!("use crate::{module}::{name};")
        } else {
            format!("use crate::{module}::{{{}}};", names.join(", "))
        }
    }

    fn render_decl(&self, decl: &Decl) -> String {
        match decl {
            Decl::Interface(interface) => {
                let fields: String = interface
                    .fields
                    .iter()
                    .map(|f| self.render_field(f))
                    .collect();
                format!(
                    "{DERIVES}\npub struct {} {{\n{fields}}}",
                    interface.name.name
                )
            }
            Decl::Function(function) => self.render_function(function),
            Decl::Alias(alias) => {
                format!("pub type {} = {};", alias.name.name, alias.value)
            }
            Decl::Raw(raw) => raw.text.clone(),
            // The open enum and the tagged union are emitted as verbatim Raw items
            // (they need hand-written serde impls), and the operation stub belongs
            // to the runtime phase; none reach render through these arms.
            Decl::Enum(_) | Decl::Union(_) | Decl::Method(_) => String::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codegen::symbol::Symbol;
    use crate::codegen::tree::{Alias, EnumDecl, Function, Interface, Method, Raw, UnionDecl};

    fn field(name: &str, ty: TypeExpr, nullable: bool, wire: Option<&str>) -> Field {
        Field {
            name: Symbol::builtin(name),
            ty,
            nullable,
            wire: wire.map(str::to_string),
        }
    }

    #[test]
    fn imports_render_as_crate_paths() {
        assert_eq!(
            RustRules.render_import("payments", &["Charge"]),
            "use crate::payments::Charge;"
        );
        // Several names from one module group into a single braced use.
        assert_eq!(
            RustRules.render_import("payments", &["Card", "Charge"]),
            "use crate::payments::{Card, Charge};"
        );
    }

    #[test]
    fn a_struct_renders_derives_and_public_fields() {
        let decl = Decl::Interface(Interface {
            name: Symbol::builtin("Charge"),
            fields: vec![field(
                "id",
                TypeExpr::Ref(Symbol::builtin("String")),
                false,
                None,
            )],
        });
        assert_eq!(
            RustRules.render_decl(&decl),
            "#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]\n\
             pub struct Charge {\n    pub id: String,\n}"
        );
    }

    #[test]
    fn a_wire_override_becomes_a_serde_rename_without_touching_the_identifier() {
        let decl = Decl::Interface(Interface {
            name: Symbol::builtin("Charge"),
            fields: vec![field(
                "memo_text",
                TypeExpr::Ref(Symbol::builtin("String")),
                false,
                Some("memo"),
            )],
        });
        let out = RustRules.render_decl(&decl);
        assert!(out.contains("    #[serde(rename = \"memo\")]\n"));
        assert!(out.contains("    pub memo_text: String,\n"));
    }

    #[test]
    fn a_wide_integer_field_routes_through_the_string_codec() {
        let decl = Decl::Interface(Interface {
            name: Symbol::builtin("Charge"),
            fields: vec![
                field(
                    "amount_cents",
                    TypeExpr::Ref(Symbol::builtin("i64")),
                    false,
                    Some("amount"),
                ),
                field(
                    "blob",
                    TypeExpr::Ref(Symbol::builtin("Vec<u8>")),
                    false,
                    None,
                ),
                field("tip", TypeExpr::Ref(Symbol::builtin("u64")), true, None),
            ],
        });
        let out = RustRules.render_decl(&decl);
        // The wire rename and the string codec combine into one serde attribute.
        assert!(out.contains("    #[serde(rename = \"amount\", with = \"i64_string\")]\n"));
        assert!(out.contains("    pub amount_cents: i64,\n"));
        // Bytes route through the base64 codec.
        assert!(out.contains("    #[serde(with = \"base64_bytes\")]\n"));
        // A nullable wide integer routes through the option submodule.
        assert!(out.contains(
            "    #[serde(default, skip_serializing_if = \"Option::is_none\", with = \"u64_string::option\")]\n"
        ));
        assert!(out.contains("    pub tip: Option<u64>,\n"));
    }

    #[test]
    fn an_optional_field_is_an_option_that_is_skipped_and_defaulted() {
        let decl = Decl::Interface(Interface {
            name: Symbol::builtin("Charge"),
            fields: vec![field(
                "note",
                TypeExpr::Ref(Symbol::builtin("String")),
                true,
                None,
            )],
        });
        let out = RustRules.render_decl(&decl);
        assert!(out.contains("    #[serde(default, skip_serializing_if = \"Option::is_none\")]\n"));
        assert!(out.contains("    pub note: Option<String>,\n"));
    }

    #[test]
    fn an_optional_renamed_field_combines_both_attributes() {
        let decl = Decl::Interface(Interface {
            name: Symbol::builtin("Charge"),
            fields: vec![field(
                "note",
                TypeExpr::Ref(Symbol::builtin("String")),
                true,
                Some("memo"),
            )],
        });
        let out = RustRules.render_decl(&decl);
        assert!(out.contains(
            "    #[serde(rename = \"memo\", default, skip_serializing_if = \"Option::is_none\")]\n"
        ));
    }

    #[test]
    fn type_expressions_render_idiomatically() {
        let rules = RustRules;
        assert_eq!(
            rules.render_type(&TypeExpr::list(TypeExpr::Ref(Symbol::builtin("Charge")))),
            "Vec<Charge>"
        );
        assert_eq!(
            rules.render_type(&TypeExpr::map(
                TypeExpr::Ref(Symbol::builtin("String")),
                TypeExpr::Ref(Symbol::builtin("Charge")),
            )),
            "std::collections::HashMap<String, Charge>"
        );
        assert_eq!(
            rules.render_type(&TypeExpr::nullable(TypeExpr::Ref(Symbol::builtin(
                "Charge"
            )))),
            "Option<Charge>"
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
                TypeExpr::Ref(Symbol::builtin("i32")),
                TypeExpr::Ref(Symbol::builtin("String")),
            )),
            "Vec<(i32, String)>"
        );
    }

    #[test]
    fn a_function_renders_with_its_signature_and_body() {
        let function = Decl::Function(Function {
            name: Symbol::builtin("decode_i64"),
            params: vec![field(
                "s",
                TypeExpr::Ref(Symbol::builtin("&str")),
                false,
                None,
            )],
            ret: Some(TypeExpr::Ref(Symbol::builtin("i64"))),
            body: FnBody::Raw {
                text: "    s.parse().unwrap()".into(),
                refs: vec![],
            },
        });
        assert_eq!(
            RustRules.render_decl(&function),
            "pub fn decode_i64(s: &str) -> i64 {\n    s.parse().unwrap()\n}"
        );
    }

    #[test]
    fn an_alias_renders_as_a_type_definition() {
        let alias = Decl::Alias(Alias {
            name: Symbol::builtin("Uuid"),
            value: "String".into(),
        });
        assert_eq!(RustRules.render_decl(&alias), "pub type Uuid = String;");
    }

    #[test]
    fn a_raw_item_renders_verbatim() {
        let raw = Decl::Raw(Raw {
            text: "impl Charge {}".into(),
            refs: vec![],
        });
        assert_eq!(RustRules.render_decl(&raw), "impl Charge {}");
    }

    #[test]
    fn enum_union_and_method_arms_render_nothing_here() {
        // The Rust target emits enums and unions as Raw items and operation stubs
        // in the runtime phase, so these declaration arms are never the rendering
        // path; they yield empty text.
        let enum_decl = Decl::Enum(EnumDecl {
            name: Symbol::builtin("Status"),
            members: vec![Symbol::builtin("pending")],
            backing: crate::codegen::tree::EnumRepr::String,
        });
        let union_decl = Decl::Union(UnionDecl {
            name: Symbol::builtin("Method"),
            discriminator: "type".into(),
            variants: vec![],
        });
        let method = Decl::Method(Method {
            name: Symbol::builtin("ping"),
            params: vec![],
            ret: None,
        });
        assert_eq!(RustRules.render_decl(&enum_decl), "");
        assert_eq!(RustRules.render_decl(&union_decl), "");
        assert_eq!(RustRules.render_decl(&method), "");
    }
}
