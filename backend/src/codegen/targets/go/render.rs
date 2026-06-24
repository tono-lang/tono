//! The Go render rules: how the shared component tree turns into Go surface
//! syntax. Structs are clean — no json tags, no marshal methods — since all wire
//! knowledge lives in the separate codec layer; an optional scalar or reference
//! becomes a pointer so it can be absent. Enums render as a named string type plus
//! its constants.
//!
//! Unions are not rendered from `Decl::Union`: Go has no sum type, so the codec
//! phase emits a sealed interface plus one wrapper struct per variant as verbatim
//! `Decl::Raw` items. That arm renders nothing here.

use crate::codegen::casing::{transform, CaseStyle, CasingConfig};
use crate::codegen::symbol::{Import, SymbolKind};
use crate::codegen::syntax::{self, TypeSyntax};
use crate::codegen::target::RenderRules;
use crate::codegen::tree::{Decl, EnumDecl, Field, FnBody, Function, TypeExpr};

/// The Go render rules.
pub struct GoRules;

/// The Go spelling of each composite type construct; the recursion lives in the
/// shared `syntax` driver. An `@entries` map is a slice of the generated generic
/// `Entry[K, V]`, which marshals each pair as a two-element array.
impl TypeSyntax for GoRules {
    fn list(&self, inner: &str) -> String {
        format!("[]{inner}")
    }
    fn map(&self, key: &str, value: &str) -> String {
        format!("map[{key}]{value}")
    }
    fn nullable(&self, inner: &str) -> String {
        format!("*{inner}")
    }
    fn generic(&self, name: &str, args: &[String]) -> String {
        format!("{name}[{}]", args.join(", "))
    }
    fn entries(&self, key: &str, value: &str) -> String {
        format!("[]Entry[{key}, {value}]")
    }
}

impl GoRules {
    fn render_type(&self, ty: &TypeExpr) -> String {
        syntax::render_type(ty, self)
    }

    fn render_field(&self, field: &Field) -> String {
        let collection = matches!(field.ty, TypeExpr::List(_) | TypeExpr::Map(_, _));
        let base = self.render_type(&field.ty);
        // An optional scalar or reference becomes a pointer so it can be absent; a
        // collection is already nullable, so it stays a slice/map.
        let ty = if field.nullable && !collection {
            format!("*{base}")
        } else {
            base
        };
        // The struct is clean: no json tag. The wire key and all wire encoding live
        // in the codec layer.
        format!("\t{} {ty}\n", field.name.name)
    }

    fn render_enum(&self, decl: &EnumDecl) -> String {
        let name = &decl.name.name;
        let mut out = format!("type {name} string\n");
        if decl.members.is_empty() {
            return out;
        }
        out.push_str("\nconst (\n");
        let pascal = CasingConfig::new(CaseStyle::Pascal);
        for member in &decl.members {
            let value = &member.name;
            let ident = format!(
                "{name}{}",
                transform(value, SymbolKind::Variant, &pascal, None)
            );
            out.push_str(&format!("\t{ident} {name} = \"{value}\"\n"));
        }
        out.push(')');
        out
    }

    fn render_function(&self, function: &Function) -> String {
        let params: Vec<String> = function
            .params
            .iter()
            .map(|p| format!("{} {}", p.name.name, self.render_type(&p.ty)))
            .collect();
        let ret = function
            .ret
            .as_ref()
            .map(|r| format!(" {}", self.render_type(r)))
            .unwrap_or_default();
        let FnBody::Raw { text, .. } = &function.body;
        format!(
            "func {}({}){ret} {{\n{text}\n}}",
            function.name.name,
            params.join(", ")
        )
    }
}

impl RenderRules for GoRules {
    fn render_import(&self, import: &Import) -> String {
        format!("import \"{}\"", import.module)
    }

    fn render_decl(&self, decl: &Decl) -> String {
        match decl {
            Decl::Interface(interface) => {
                let fields: String = interface
                    .fields
                    .iter()
                    .map(|f| self.render_field(f))
                    .collect();
                format!("type {} struct {{\n{fields}}}", interface.name.name)
            }
            Decl::Enum(decl) => self.render_enum(decl),
            Decl::Function(function) => self.render_function(function),
            // A branded well-known type is a named string.
            Decl::Alias(alias) => format!("type {} {}", alias.name.name, alias.value),
            Decl::Raw(raw) => raw.text.clone(),
            // The union (a struct + hand-written JSON methods) is emitted as a Raw
            // item, and the operation stub belongs to the runtime phase; neither
            // reaches render through these arms.
            Decl::Union(_) | Decl::Method(_) => String::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codegen::symbol::Symbol;
    use crate::codegen::tree::{Alias, Function, Interface, Method, Raw, UnionDecl};

    fn field(name: &str, ty: TypeExpr, nullable: bool, wire: &str) -> Field {
        Field {
            name: Symbol::builtin(name),
            ty,
            nullable,
            wire: Some(wire.to_string()),
        }
    }

    #[test]
    fn imports_render_as_go_import_lines() {
        assert_eq!(
            GoRules.render_import(&Import {
                module: "payments".into(),
                imported: "Charge".into(),
            }),
            "import \"payments\""
        );
    }

    #[test]
    fn a_struct_renders_clean_fields_without_json_tags() {
        let decl = Decl::Interface(Interface {
            name: Symbol::builtin("Charge"),
            fields: vec![
                field(
                    "AccountID",
                    TypeExpr::Ref(Symbol::builtin("int64")),
                    false,
                    "account_id",
                ),
                field(
                    "Note",
                    TypeExpr::Ref(Symbol::builtin("string")),
                    true,
                    "note",
                ),
            ],
        });
        let out = GoRules.render_decl(&decl);
        assert!(out.starts_with("type Charge struct {\n"));
        // No json tags; the 64-bit integer is held natively.
        assert!(out.contains("\tAccountID int64\n"));
        assert!(!out.contains("json:"));
        // An optional scalar becomes a pointer.
        assert!(out.contains("\tNote *string\n"));
    }

    #[test]
    fn collections_stay_slices_and_maps_even_when_optional() {
        let decl = Decl::Interface(Interface {
            name: Symbol::builtin("Bag"),
            fields: vec![
                field(
                    "Tags",
                    TypeExpr::list(TypeExpr::Ref(Symbol::builtin("string"))),
                    true,
                    "tags",
                ),
                field(
                    "Meta",
                    TypeExpr::map(
                        TypeExpr::Ref(Symbol::builtin("string")),
                        TypeExpr::Ref(Symbol::builtin("int32")),
                    ),
                    false,
                    "meta",
                ),
            ],
        });
        let out = GoRules.render_decl(&decl);
        // An optional slice is not a pointer; it stays a slice.
        assert!(out.contains("\tTags []string\n"));
        assert!(out.contains("\tMeta map[string]int32\n"));
    }

    #[test]
    fn an_enum_renders_a_named_string_and_its_constants() {
        let decl = Decl::Enum(EnumDecl {
            name: Symbol::builtin("Status"),
            members: vec![Symbol::builtin("pending"), Symbol::builtin("in_review")],
        });
        assert_eq!(
            GoRules.render_decl(&decl),
            "type Status string\n\nconst (\n\tStatusPending Status = \"pending\"\n\t\
             StatusInReview Status = \"in_review\"\n)"
        );
    }

    #[test]
    fn an_empty_enum_is_just_the_named_string() {
        let decl = Decl::Enum(EnumDecl {
            name: Symbol::builtin("Empty"),
            members: vec![],
        });
        assert_eq!(GoRules.render_decl(&decl), "type Empty string\n");
    }

    #[test]
    fn type_expressions_render_idiomatically() {
        let rules = GoRules;
        assert_eq!(
            rules.render_type(&TypeExpr::list(TypeExpr::Ref(Symbol::builtin("Charge")))),
            "[]Charge"
        );
        assert_eq!(
            rules.render_type(&TypeExpr::map(
                TypeExpr::Ref(Symbol::builtin("string")),
                TypeExpr::Ref(Symbol::builtin("Charge")),
            )),
            "map[string]Charge"
        );
        assert_eq!(
            rules.render_type(&TypeExpr::nullable(TypeExpr::Ref(Symbol::builtin(
                "Charge"
            )))),
            "*Charge"
        );
        assert_eq!(
            rules.render_type(&TypeExpr::Generic(
                Symbol::builtin("Page"),
                vec![TypeExpr::Ref(Symbol::builtin("Charge"))],
            )),
            "Page[Charge]"
        );
        assert_eq!(
            rules.render_type(&TypeExpr::entries(
                TypeExpr::Ref(Symbol::builtin("int32")),
                TypeExpr::Ref(Symbol::builtin("string")),
            )),
            "[]Entry[int32, string]"
        );
    }

    #[test]
    fn a_function_renders_with_its_signature_and_body() {
        let function = Decl::Function(Function {
            name: Symbol::builtin("Decode"),
            params: vec![field(
                "data",
                TypeExpr::Ref(Symbol::builtin("[]byte")),
                false,
                "data",
            )],
            ret: Some(TypeExpr::Ref(Symbol::builtin("error"))),
            body: FnBody::Raw {
                text: "\treturn nil".into(),
                refs: vec![],
            },
        });
        assert_eq!(
            GoRules.render_decl(&function),
            "func Decode(data []byte) error {\n\treturn nil\n}"
        );
    }

    #[test]
    fn an_alias_renders_as_a_named_type() {
        let alias = Decl::Alias(Alias {
            name: Symbol::builtin("Uuid"),
            value: "string".into(),
        });
        assert_eq!(GoRules.render_decl(&alias), "type Uuid string");
    }

    #[test]
    fn a_raw_item_renders_verbatim() {
        let raw = Decl::Raw(Raw {
            text: "func (m Method) foo() {}".into(),
            refs: vec![],
        });
        assert_eq!(GoRules.render_decl(&raw), "func (m Method) foo() {}");
    }

    #[test]
    fn union_and_method_arms_render_nothing_here() {
        let union = Decl::Union(UnionDecl {
            name: Symbol::builtin("Method"),
            discriminator: "type".into(),
            variants: vec![],
        });
        let method = Decl::Method(Method {
            name: Symbol::builtin("Ping"),
            params: vec![],
            ret: None,
        });
        assert_eq!(GoRules.render_decl(&union), "");
        assert_eq!(GoRules.render_decl(&method), "");
    }
}
