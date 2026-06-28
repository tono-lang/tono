//! The Go render rules: how the shared component tree turns into Go surface
//! syntax. A struct field carries an `encoding/json` tag — the wire key, plus
//! `,string` for a 64-bit integer (held natively but serialized as a string) and
//! `,omitempty` for an optional pointer — so `encoding/json` does the wire work
//! natively; an optional scalar or reference becomes a pointer so it can be absent.
//! Enums render as a named string type plus its constants, which `encoding/json`
//! serializes natively.
//!
//! Unions are not rendered from `Decl::Union`: Go has no sum type, so the codec
//! phase emits an interface plus one wrapper struct per variant as verbatim
//! `Decl::Raw` items. That arm renders nothing here.

use crate::codegen::casing::{transform, CaseStyle, CasingConfig};
use crate::codegen::symbol::SymbolKind;
use crate::codegen::syntax::{self, TypeSyntax};
use crate::codegen::target::RenderRules;
use crate::codegen::tree::{Decl, EnumDecl, Field, FnBody, Function, TypeExpr};

/// The Go render rules.
pub struct GoRules;

/// The Go spelling of each composite type construct; the recursion lives in the
/// shared `syntax` driver. An `@entries` map is the generated generic `Entries[K,
/// V]`, whose `MarshalJSON`/`UnmarshalJSON` carry each pair as a two-element array.
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
        format!("Entries[{key}, {value}]")
    }
}

/// Whether a field's top-level type is a 64-bit integer, which `encoding/json`
/// must serialize as a string (the `,string` tag option) to stay precise above
/// 2^53. The check is on the field's own type only: a 64-bit integer nested inside
/// a collection or map is an unexercised edge case the `,string` option cannot
/// reach (it does not recurse into elements), and is left for a future need.
fn is_wide_int(ty: &TypeExpr) -> bool {
    matches!(ty, TypeExpr::Ref(sym) if sym.name == "int64" || sym.name == "uint64")
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
        let pointer = field.nullable && !collection;
        let ty = if pointer { format!("*{base}") } else { base };
        // The `encoding/json` struct tag carries all the wire work: the wire key,
        // `,string` for a 64-bit integer (precise above 2^53), and `,omitempty` for
        // an optional pointer so an absent value is dropped.
        let wire = field.wire.as_deref().unwrap_or(&field.name.name);
        let mut tag = wire.to_string();
        if is_wide_int(&field.ty) {
            tag.push_str(",string");
        }
        if pointer {
            tag.push_str(",omitempty");
        }
        format!("\t{} {ty} `json:\"{tag}\"`\n", field.name.name)
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
    fn render_import(&self, module: &str, _names: &[&str]) -> String {
        // Go imports the whole package, so the per-symbol names play no part.
        format!("import \"{module}\"")
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
        // Go imports the whole package, so the per-symbol names are ignored.
        assert_eq!(
            GoRules.render_import("payments", &["Charge", "Card"]),
            "import \"payments\""
        );
    }

    #[test]
    fn a_struct_renders_fields_with_json_tags() {
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
                field("Tip", TypeExpr::Ref(Symbol::builtin("int64")), true, "tip"),
                field(
                    "Secret",
                    TypeExpr::Ref(Symbol::builtin("[]byte")),
                    false,
                    "secret",
                ),
            ],
        });
        let out = GoRules.render_decl(&decl);
        assert!(out.starts_with("type Charge struct {\n"));
        // The 64-bit integer is held natively but tagged `,string`.
        assert!(out.contains("\tAccountID int64 `json:\"account_id,string\"`\n"));
        // An optional scalar becomes a pointer with `,omitempty`.
        assert!(out.contains("\tNote *string `json:\"note,omitempty\"`\n"));
        // An optional 64-bit integer combines `,string` and `,omitempty`.
        assert!(out.contains("\tTip *int64 `json:\"tip,string,omitempty\"`\n"));
        // `bytes` is a plain tag; `encoding/json` base64-encodes a []byte natively.
        assert!(out.contains("\tSecret []byte `json:\"secret\"`\n"));
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
        // An optional slice is not a pointer; it stays a slice, with no omitempty.
        assert!(out.contains("\tTags []string `json:\"tags\"`\n"));
        assert!(out.contains("\tMeta map[string]int32 `json:\"meta\"`\n"));
    }

    #[test]
    fn a_field_without_a_wire_override_tags_with_its_name() {
        let decl = Decl::Interface(Interface {
            name: Symbol::builtin("Charge"),
            fields: vec![Field {
                name: Symbol::builtin("Id"),
                ty: TypeExpr::Ref(Symbol::builtin("string")),
                nullable: false,
                wire: None,
            }],
        });
        assert!(GoRules
            .render_decl(&decl)
            .contains("\tId string `json:\"Id\"`\n"));
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
            "Entries[int32, string]"
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
