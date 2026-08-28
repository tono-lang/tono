//! The Rust render rules: how the shared component tree turns into Rust surface
//! syntax. Structs carry serde derives and per-field attributes; the wire key
//! rides `#[serde(rename)]` and an optional field becomes `Option<T>` that is
//! skipped on serialize and defaulted on deserialize.
//!
//! Enums and unions are not rendered from `Decl::Enum`/`Decl::Union`: the open
//! enum needs a hand-written `Deserialize` (for its catch-all `Unknown` arm) and
//! a tagged union needs custom plumbing, so the Rust target emits both as
//! verbatim `Decl::Raw` items in a later phase. Their arms here render nothing.

use crate::codegen::doc;
use crate::codegen::syntax::{self, TypeSyntax};
use crate::codegen::target::RenderRules;
use crate::codegen::targets::rust::codecs::serde_with;
use crate::codegen::tree::{Decl, Field, FnBody, Function, Method, TypeExpr};

/// The standard derives every generated struct and enum carries.
const DERIVES: &str = "#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]";

/// The `#[deprecated]` attribute for a `@deprecated` element, or empty when the
/// element is not deprecated. A `Some("")` (marked without a reason) renders the
/// bare form. Backslashes and quotes in the note are escaped, and any whitespace
/// control char collapses to a space so the attribute stays a single valid line;
/// the caller adds the indentation and trailing newline for its position.
///
/// No `#[allow(deprecated)]` is emitted on the generated serde impls: the derived
/// `Serialize`/`Deserialize` accessing a deprecated field compile clean even under
/// `deny(warnings)`, verified by the example-SDK compile gate which now builds with
/// `-D warnings`. The warning fires only at external call sites, which is the point
/// of `@deprecated`.
pub(crate) fn deprecated_attr(reason: Option<&str>) -> String {
    match reason {
        None => String::new(),
        Some("") => "#[deprecated]".into(),
        Some(r) => {
            let note = r
                .replace('\\', "\\\\")
                .replace('"', "\\\"")
                .replace(['\n', '\r', '\t'], " ");
            format!("#[deprecated(note = \"{note}\")]")
        }
    }
}

/// Prefix a rendered line/block with a deprecation attribute, indented and
/// newline-terminated, when one is present.
fn deprecated_prefix(reason: Option<&str>, indent: &str) -> String {
    let attr = deprecated_attr(reason);
    if attr.is_empty() {
        String::new()
    } else {
        format!("{indent}{attr}\n")
    }
}

/// Render a component-tree type expression into Rust surface syntax. Free so the
/// codec layer (which builds union payload types) can reuse it.
pub(crate) fn type_string(ty: &TypeExpr) -> String {
    syntax::render_type(ty, &RustRules::default())
}

/// The Rust render rules.
///
/// `crate_visible` renders a group Rust does not move: an internal group rides
/// the module's own file, so each item it declares says `pub(crate)` instead of
/// `pub`. That is how Rust expresses "part of this module, not part of its
/// surface", and it is why no Rust file here is named for its audience.
#[derive(Default)]
pub struct RustRules {
    pub crate_visible: bool,
}

/// The visibility an item is declared with, and the lint allowance that goes
/// with it. Only the item head carries either, so a rendered declaration says
/// them once.
///
/// A crate-visible declaration exists for the SDK's own use, so the lint that
/// wants every item reached is not the right judge of it: the file it shares
/// with the public group carries no blanket allowance.
fn vis(crate_visible: bool) -> &'static str {
    if crate_visible {
        "#[allow(dead_code)]\npub(crate)"
    } else {
        "pub"
    }
}

/// Restate the visibility of every item in verbatim source.
///
/// Only an item head sits at column zero in the Rust the emitters build (a
/// nested `pub fn` inside an `impl` is always indented), so anchoring on the
/// line start reaches exactly the declarations a reader could otherwise name.
fn restate_visibility(text: &str, crate_visible: bool) -> String {
    if !crate_visible {
        return text.to_string();
    }
    text.lines()
        .map(|line| match line.strip_prefix("pub ") {
            Some(rest) => format!("{} {rest}", vis(true)),
            None => line.to_string(),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// The generic type-parameter clause of a definition (`<T>`, `<T, U>`), or the
/// empty string for a non-generic shape. The serde derives generate the bound on
/// each parameter themselves, so the names render bare here.
fn type_params(params: &[String]) -> String {
    if params.is_empty() {
        String::new()
    } else {
        format!("<{}>", params.join(", "))
    }
}

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
        let doc = field
            .doc
            .as_deref()
            .map(|d| doc::rustdoc(d, "    "))
            .unwrap_or_default();
        let dep = deprecated_prefix(field.deprecated.as_deref(), "    ");
        format!("{doc}{dep}{attr}    pub {}: {ty},\n", field.name.name)
    }

    /// One client method signature. An async operation is an `async fn` (the
    /// caller `.await`s it); a sync one is a plain fn. The error channel rides
    /// the return type as `Result<T, E>`, the native error idiom.
    fn render_method(&self, method: &Method) -> String {
        let mut params: Vec<String> = vec!["&self".into()];
        params.extend(
            method
                .params
                .iter()
                .map(|p| format!("{}: {}", p.name.name, self.render_type(&p.ty))),
        );
        let ok = method
            .ret
            .as_ref()
            .map(|r| self.render_type(r))
            .unwrap_or_else(|| "()".into());
        let ret = match &method.err {
            Some(err) => format!("Result<{ok}, {}>", self.render_type(err)),
            None => ok,
        };
        let effect = if method.is_async { "async " } else { "" };
        let doc = method
            .doc
            .as_deref()
            .map(|d| doc::rustdoc(d, "    "))
            .unwrap_or_default();
        format!(
            "{doc}    {effect}fn {}({}) -> {ret};\n",
            method.name.name,
            params.join(", ")
        )
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
            "{vis} fn {}({}){ret} {{\n{text}\n}}",
            function.name.name,
            params.join(", "),
            vis = vis(self.crate_visible),
        )
    }
}

impl RenderRules for RustRules {
    fn render_import(&self, _from_module: &str, module: &str, names: &[&str]) -> String {
        // Rust paths are absolute from the crate root, so the importer is
        // irrelevant. A group is a module of the crate: payments.common's types
        // group is `crate::payments::common::types`.
        let path = crate::codegen::layout::rust_path(module)
            .unwrap_or_else(|| format!("crate::{}", module.replace('.', "::")));
        // A single name needs no braces; several group into one `use`.
        if let [name] = names {
            format!("use {path}::{name};")
        } else {
            format!("use {path}::{{{}}};", names.join(", "))
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
                let doc = interface
                    .doc
                    .as_deref()
                    .map(|d| doc::rustdoc(d, ""))
                    .unwrap_or_default();
                let dep = deprecated_prefix(interface.deprecated.as_deref(), "");
                format!(
                    "{doc}{dep}{DERIVES}\n{vis} struct {}{} {{\n{fields}}}",
                    interface.name.name,
                    type_params(&interface.params),
                    vis = vis(self.crate_visible),
                )
            }
            Decl::Function(function) => self.render_function(function),
            Decl::Alias(alias) => {
                format!(
                    "{} type {} = {};",
                    vis(self.crate_visible),
                    alias.name.name,
                    alias.value
                )
            }
            Decl::Raw(raw) => restate_visibility(&raw.text, self.crate_visible),
            Decl::Client(client) => {
                let methods: String = client
                    .methods
                    .iter()
                    .map(|m| self.render_method(m))
                    .collect();
                // `async fn` in a public trait lints by default (the returned
                // future's auto-trait bounds are unnamed); the allow is
                // deliberate, since refining the bounds is the implementor's
                // concern, not the generated signature's.
                let allow = if client.methods.iter().any(|m| m.is_async) {
                    "#[allow(async_fn_in_trait)]\n"
                } else {
                    ""
                };
                format!("{allow}pub trait {} {{\n{methods}}}", client.name.name)
            }
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
            tag: None,
            name: Symbol::builtin(name),
            ty,
            nullable,
            wire: wire.map(str::to_string),
            deprecated: None,
            doc: None,
        }
    }

    #[test]
    fn imports_render_as_crate_paths() {
        assert_eq!(
            RustRules::default().render_import("billing", "payments", &["Charge"]),
            "use crate::payments::Charge;"
        );
        // Several names from one module group into a single braced use.
        assert_eq!(
            RustRules::default().render_import("billing", "payments", &["Card", "Charge"]),
            "use crate::payments::{Card, Charge};"
        );
        // A dotted module becomes a nested crate path.
        assert_eq!(
            RustRules::default().render_import("payments.charges", "payments.common", &["Money"]),
            "use crate::payments::common::Money;"
        );
    }

    #[test]
    fn a_struct_renders_derives_and_public_fields() {
        let decl = Decl::Interface(Interface {
            name: Symbol::builtin("Charge"),
            params: vec![],
            fields: vec![field(
                "id",
                TypeExpr::Ref(Symbol::builtin("String")),
                false,
                None,
            )],
            deprecated: None,
            doc: None,
        });
        assert_eq!(
            RustRules::default().render_decl(&decl),
            "#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]\n\
             pub struct Charge {\n    pub id: String,\n}"
        );
    }

    #[test]
    fn a_generic_struct_renders_its_type_parameter_clause() {
        // The serde derives synthesize the per-parameter bound, so the clause is
        // just the bare names.
        let decl = Decl::Interface(Interface {
            name: Symbol::builtin("Page"),
            params: vec!["T".into()],
            fields: vec![field(
                "items",
                TypeExpr::list(TypeExpr::Ref(Symbol::builtin("T"))),
                false,
                None,
            )],
            deprecated: None,
            doc: None,
        });
        assert_eq!(
            RustRules::default().render_decl(&decl),
            "#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]\n\
             pub struct Page<T> {\n    pub items: Vec<T>,\n}"
        );
    }

    #[test]
    fn a_wire_override_becomes_a_serde_rename_without_touching_the_identifier() {
        let decl = Decl::Interface(Interface {
            name: Symbol::builtin("Charge"),
            params: vec![],
            fields: vec![field(
                "memo_text",
                TypeExpr::Ref(Symbol::builtin("String")),
                false,
                Some("memo"),
            )],
            deprecated: None,
            doc: None,
        });
        let out = RustRules::default().render_decl(&decl);
        assert!(out.contains("    #[serde(rename = \"memo\")]\n"));
        assert!(out.contains("    pub memo_text: String,\n"));
    }

    #[test]
    fn a_wide_integer_field_routes_through_the_string_codec() {
        let decl = Decl::Interface(Interface {
            name: Symbol::builtin("Charge"),
            params: vec![],
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
            deprecated: None,
            doc: None,
        });
        let out = RustRules::default().render_decl(&decl);
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
            params: vec![],
            fields: vec![field(
                "note",
                TypeExpr::Ref(Symbol::builtin("String")),
                true,
                None,
            )],
            deprecated: None,
            doc: None,
        });
        let out = RustRules::default().render_decl(&decl);
        assert!(out.contains("    #[serde(default, skip_serializing_if = \"Option::is_none\")]\n"));
        assert!(out.contains("    pub note: Option<String>,\n"));
    }

    #[test]
    fn an_optional_renamed_field_combines_both_attributes() {
        let decl = Decl::Interface(Interface {
            name: Symbol::builtin("Charge"),
            params: vec![],
            fields: vec![field(
                "note",
                TypeExpr::Ref(Symbol::builtin("String")),
                true,
                Some("memo"),
            )],
            deprecated: None,
            doc: None,
        });
        let out = RustRules::default().render_decl(&decl);
        assert!(out.contains(
            "    #[serde(rename = \"memo\", default, skip_serializing_if = \"Option::is_none\")]\n"
        ));
    }

    #[test]
    fn a_deprecated_struct_and_field_carry_rust_attributes() {
        let decl = Decl::Interface(Interface {
            name: Symbol::builtin("Charge"),
            params: vec![],
            fields: vec![
                Field {
                    tag: None,
                    name: Symbol::builtin("amount"),
                    ty: TypeExpr::Ref(Symbol::builtin("u32")),
                    nullable: false,
                    wire: None,
                    deprecated: Some("use amount_cents".into()),
                    doc: None,
                },
                Field {
                    tag: None,
                    name: Symbol::builtin("id"),
                    ty: TypeExpr::Ref(Symbol::builtin("String")),
                    nullable: false,
                    wire: None,
                    deprecated: Some(String::new()),
                    doc: None,
                },
            ],
            deprecated: Some("use ChargeV2".into()),
            doc: None,
        });
        let out = RustRules::default().render_decl(&decl);
        // The struct attribute leads the derive so it attaches to the type.
        assert!(out.contains("#[deprecated(note = \"use ChargeV2\")]\n#[derive("));
        // A field with a reason carries the note; a bare one the plain attribute.
        assert!(
            out.contains("    #[deprecated(note = \"use amount_cents\")]\n    pub amount: u32,")
        );
        assert!(out.contains("    #[deprecated]\n    pub id: String,"));
    }

    #[test]
    fn doc_renders_as_rustdoc_on_the_struct_and_the_field() {
        let decl = Decl::Interface(Interface {
            name: Symbol::builtin("Charge"),
            params: vec![],
            fields: vec![Field {
                tag: None,
                name: Symbol::builtin("amount"),
                ty: TypeExpr::Ref(Symbol::builtin("u32")),
                nullable: false,
                wire: None,
                deprecated: None,
                doc: Some("The amount in **minor** units.".into()),
            }],
            deprecated: None,
            doc: Some("A billing charge.\n\nMarkdown rides rustdoc verbatim.".into()),
        });
        let out = RustRules::default().render_decl(&decl);
        // The struct doc leads, one `///` per line (a blank line stays a bare `///`).
        assert!(out.starts_with(
            "/// A billing charge.\n///\n/// Markdown rides rustdoc verbatim.\n#[derive("
        ));
        // The field doc sits above the field (a `u32` needs no serde codec, so the
        // doc is adjacent), Markdown untouched.
        assert!(out.contains("    /// The amount in **minor** units.\n    pub amount: u32,"));
    }

    #[test]
    fn type_expressions_render_idiomatically() {
        let rules = RustRules::default();
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
            RustRules::default().render_decl(&function),
            "pub fn decode_i64(s: &str) -> i64 {\n    s.parse().unwrap()\n}"
        );
    }

    #[test]
    fn an_alias_renders_as_a_type_definition() {
        let alias = Decl::Alias(Alias {
            name: Symbol::builtin("Uuid"),
            value: "String".into(),
        });
        assert_eq!(
            RustRules::default().render_decl(&alias),
            "pub type Uuid = String;"
        );
    }

    #[test]
    fn a_raw_item_renders_verbatim() {
        let raw = Decl::Raw(Raw {
            text: "impl Charge {}".into(),
            refs: vec![],
            ..Raw::default()
        });
        assert_eq!(RustRules::default().render_decl(&raw), "impl Charge {}");
    }

    #[test]
    fn enum_union_and_method_arms_render_nothing_here() {
        // The Rust target emits enums and unions as Raw items and operation stubs
        // in the runtime phase, so these declaration arms are never the rendering
        // path; they yield empty text.
        let enum_decl = Decl::Enum(EnumDecl {
            name: Symbol::builtin("Status"),
            members: vec![Symbol::builtin("pending")],
            member_docs: vec![None],
            backing: crate::codegen::tree::EnumRepr::String,
            deprecated: None,
            doc: None,
        });
        let union_decl = Decl::Union(UnionDecl {
            name: Symbol::builtin("Method"),
            discriminator: "type".into(),
            variants: vec![],
            deprecated: None,
            doc: None,
        });
        let method = Decl::Method(Method {
            name: Symbol::builtin("ping"),
            params: vec![],
            ret: None,
            err: None,
            is_async: false,
            doc: None,
        });
        assert_eq!(RustRules::default().render_decl(&enum_decl), "");
        assert_eq!(RustRules::default().render_decl(&union_decl), "");
        assert_eq!(RustRules::default().render_decl(&method), "");
    }

    #[test]
    fn a_client_renders_a_trait_with_async_and_result_signatures() {
        let decl = Decl::Client(crate::codegen::tree::ClientDecl {
            name: Symbol::builtin("Client"),
            methods: vec![
                Method {
                    name: Symbol::builtin("create_charge"),
                    params: vec![field(
                        "input",
                        TypeExpr::Ref(Symbol::builtin("CreateChargeInput")),
                        false,
                        None,
                    )],
                    ret: Some(TypeExpr::Ref(Symbol::builtin("Charge"))),
                    err: Some(TypeExpr::Ref(Symbol::builtin("TonoError"))),
                    is_async: true,
                    doc: None,
                },
                Method {
                    name: Symbol::builtin("local_op"),
                    params: vec![],
                    ret: None,
                    err: Some(TypeExpr::Ref(Symbol::builtin("TonoError"))),
                    is_async: false,
                    doc: None,
                },
            ],
        });
        assert_eq!(
            RustRules::default().render_decl(&decl),
            "#[allow(async_fn_in_trait)]\npub trait Client {\n    async fn \
             create_charge(&self, input: CreateChargeInput) -> Result<Charge, \
             TonoError>;\n    fn local_op(&self) -> Result<(), TonoError>;\n}"
        );
    }

    #[test]
    fn a_client_with_no_async_method_needs_no_allow_attribute() {
        let decl = Decl::Client(crate::codegen::tree::ClientDecl {
            name: Symbol::builtin("Client"),
            methods: vec![Method {
                name: Symbol::builtin("local_op"),
                params: vec![],
                ret: None,
                err: Some(TypeExpr::Ref(Symbol::builtin("TonoError"))),
                is_async: false,
                doc: None,
            }],
        });
        assert!(!RustRules::default()
            .render_decl(&decl)
            .contains("async_fn_in_trait"));
    }
}
