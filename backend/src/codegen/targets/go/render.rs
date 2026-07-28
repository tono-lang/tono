//! The Go render rules: how the shared component tree turns into Go surface
//! syntax. A struct field carries an `encoding/json` tag — the wire key, plus
//! `,string` for a 64-bit integer (held natively but serialized as a string) and
//! `,omitempty` for an optional pointer — so `encoding/json` does the wire work
//! natively; an optional scalar or reference becomes a pointer so it can be absent.
//! Enums render as a named string or int type plus its constants, which
//! `encoding/json` serializes natively.
//!
//! Unions are not rendered from `Decl::Union`: Go has no sum type, so the codec
//! phase emits an interface plus one wrapper struct per variant as verbatim
//! `Decl::Raw` items. That arm renders nothing here.

use crate::codegen::casing::{transform, CaseStyle, CasingConfig};
use crate::codegen::doc;
use crate::codegen::layout;
use crate::codegen::symbol::SymbolKind;
use crate::codegen::syntax::{self, TypeSyntax};
use crate::codegen::target::RenderRules;
use crate::codegen::tree::{Decl, EnumDecl, EnumRepr, Field, FnBody, Function, Method, TypeExpr};

/// Whether `s` is a legal Go identifier (so Go can infer a package selector from
/// an import path segment). A hyphenated segment like `http-go` is not, and needs
/// an explicit import alias.
fn is_go_ident(s: &str) -> bool {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) if c == '_' || c.is_ascii_alphabetic() => {
            chars.all(|c| c == '_' || c.is_ascii_alphanumeric())
        }
        _ => false,
    }
}

/// A godoc comment prefix for a documented element, indented and newline-terminated,
/// or empty when there is no doc. The Markdown is flattened to plain text (godoc is
/// not Markdown) and sits directly above the declaration.
fn doc_prefix(doc: Option<&str>, indent: &str) -> String {
    doc.map(|d| doc::godoc(d, indent)).unwrap_or_default()
}

/// The godoc deprecation comment for a `@deprecated` element, or empty when it is
/// not deprecated. Go's convention is a `// Deprecated: ...` line directly above
/// the declaration (no blank line between, or gofmt detaches it); a `Some("")`
/// (marked without a reason) renders the bare `// Deprecated:`. A newline in the
/// reason collapses to a space so the comment stays one line. The caller adds the
/// indentation and trailing newline for its position.
pub(crate) fn deprecated_comment(reason: Option<&str>) -> String {
    match reason {
        None => String::new(),
        Some("") => "// Deprecated:".into(),
        Some(r) => format!("// Deprecated: {}", r.replace('\n', " ")),
    }
}

/// Prefix a declaration/field with its godoc deprecation comment, indented and
/// newline-terminated, when one is present.
fn deprecated_prefix(reason: Option<&str>, indent: &str) -> String {
    let comment = deprecated_comment(reason);
    if comment.is_empty() {
        String::new()
    } else {
        format!("{indent}{comment}\n")
    }
}

/// The Go render rules. `go_module` is the SDK's Go module path (from
/// `--go-module`); a cross-package import to one of the SDK's own groups needs it
/// as a prefix, since Go has no relative imports. Standard-library imports
/// (`encoding/json`, `fmt`) are not the SDK's, so they are never prefixed.
#[derive(Default)]
pub struct GoRules {
    pub go_module: Option<String>,
    /// The group path of the file being rendered, so a reference to another
    /// package gets a `pkg.` selector while a same-package one stays bare. A
    /// module's groups are files of one package, so only a reference out of the
    /// module (or into the SDK's shared package) qualifies.
    pub current: String,
}

/// The generic type-parameter clause of a definition (`[T any]`, `[T any, U any]`),
/// or the empty string for a non-generic shape. Go requires a constraint on every
/// type parameter, and an unconstrained parameter is spelled `any`.
fn type_params(params: &[String]) -> String {
    if params.is_empty() {
        String::new()
    } else {
        let bound: Vec<String> = params.iter().map(|p| format!("{p} any")).collect();
        format!("[{}]", bound.join(", "))
    }
}

/// The Go spelling of each composite type construct; the recursion lives in the
/// shared `syntax` driver. An `@entries` map is the generated generic `Entries[K,
/// V]`, whose `MarshalJSON`/`UnmarshalJSON` carry each pair as a two-element array.
/// Renders a type into opaque text: an imported leaf becomes a slot, so which
/// package it resolves through is decided when the file is rendered rather than
/// baked in where the text is built.
pub struct SlotRules;

impl TypeSyntax for SlotRules {
    fn leaf(&self, symbol: &crate::codegen::symbol::Symbol) -> String {
        match symbol.import {
            Some(_) => crate::codegen::tree::symbol_slot(&symbol.name),
            None => symbol.name.clone(),
        }
    }
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

impl TypeSyntax for GoRules {
    fn leaf(&self, symbol: &crate::codegen::symbol::Symbol) -> String {
        // A reference to another SDK package is qualified with that package's
        // selector; a same-package reference and a built-in stay bare. Only the
        // SDK's own groups qualify, so a cross-module reference gets
        // `common.Status` while a sibling group of the same module, being the
        // same package, leaves the name bare.
        match &symbol.import {
            Some(import) if !layout::same_go_package(&self.current, &import.module) => {
                match layout::go_selector(&import.module) {
                    Some(pkg) => format!("{pkg}.{}", symbol.name),
                    None => symbol.name.clone(),
                }
            }
            _ => symbol.name.clone(),
        }
    }
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
        let doc = doc_prefix(field.doc.as_deref(), "\t");
        let dep = deprecated_prefix(field.deprecated.as_deref(), "\t");
        format!("{doc}{dep}\t{} {ty} `json:\"{tag}\"`\n", field.name.name)
    }

    fn render_enum(&self, decl: &EnumDecl) -> String {
        // A string-backed enum is a named `string`; an int-backed one a named
        // `int`, both serialized natively by `encoding/json`.
        let name = &decl.name.name;
        let base = match decl.backing {
            EnumRepr::String => "string",
            EnumRepr::Int(_) => "int",
        };
        let doc = doc_prefix(decl.doc.as_deref(), "");
        let dep = deprecated_prefix(decl.deprecated.as_deref(), "");
        let mut out = format!("{doc}{dep}type {name} {base}\n");
        if decl.members.is_empty() {
            return out;
        }
        out.push_str("\nconst (\n");
        let pascal = CasingConfig::new(CaseStyle::Pascal);
        for (i, member) in decl.members.iter().enumerate() {
            let value = &member.name;
            let ident = format!(
                "{name}{}",
                transform(value, SymbolKind::Variant, &pascal, None)
            );
            // Per-member docs are parallel to members; render one above its const.
            if let Some(Some(d)) = decl.member_docs.get(i) {
                out.push_str(&doc::godoc(d, "\t"));
            }
            match &decl.backing {
                EnumRepr::String => out.push_str(&format!("\t{ident} {name} = \"{value}\"\n")),
                EnumRepr::Int(ints) => out.push_str(&format!("\t{ident} {name} = {}\n", ints[i])),
            }
        }
        out.push(')');
        out
    }

    /// One client method signature. Go has no suspension marker, so an async
    /// operation lowers to the same blocking signature as a sync one (the
    /// accepted caveat: the wait point is invisible at the call site). The
    /// error channel is the native `(T, error)` pair.
    fn render_method(&self, method: &Method) -> String {
        let params: Vec<String> = method
            .params
            .iter()
            .map(|p| format!("{} {}", p.name.name, self.render_type(&p.ty)))
            .collect();
        let ret = match (&method.ret, &method.err) {
            (Some(ok), Some(_)) => format!(" ({}, error)", self.render_type(ok)),
            (Some(ok), None) => format!(" {}", self.render_type(ok)),
            (None, Some(_)) => " error".into(),
            (None, None) => String::new(),
        };
        let doc = doc_prefix(method.doc.as_deref(), "\t");
        format!("{doc}\t{}({}){ret}\n", method.name.name, params.join(", "))
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
    fn render_import(&self, _from_module: &str, module: &str, names: &[&str]) -> String {
        // Go imports the whole package, so the per-symbol names play no part. One
        // of the SDK's own groups is a package sub-path prefixed with the SDK's
        // Go module path so it resolves; a standard-library import
        // (encoding/json, fmt) and an external module path (which already carries
        // slashes, and whose dots are host-name dots) are left verbatim.
        let sdk = self
            .go_module
            .as_deref()
            .and_then(|root| layout::go_import(root, module));
        let is_internal = sdk.is_some();
        let full = match sdk {
            Some(path) => path,
            None if module.contains('/') => module.to_string(),
            None => module.replace('.', "/"),
        };
        // Go infers the package selector from the path's last segment, but only
        // when that segment is a legal identifier. The runtimes/http-go package
        // clause is `tonohttp` and `http-go` is not an identifier (the hyphen),
        // so the reference cannot resolve without an explicit alias. Emit it in
        // that case, taking the alias from the import name. A legal-identifier
        // segment (every stdlib package, every internal SDK module, the flat
        // single-package layout) stays bare, so the `imported` slot (reused for
        // the referenced symbol name in those layouts) is correctly ignored.
        let inferred = full.rsplit('/').next().unwrap_or(&full);
        match names.first() {
            Some(alias) if !is_internal && !is_go_ident(inferred) => {
                format!("import {alias} \"{full}\"")
            }
            _ => format!("import \"{full}\""),
        }
    }

    /// A reference inside opaque text: the same rule the type renderer uses, so
    /// a name from another package is qualified with its selector and a
    /// same-package one stays bare.
    fn render_symbol(&self, symbol: &crate::codegen::symbol::Symbol) -> String {
        <Self as TypeSyntax>::leaf(self, symbol)
    }

    fn render_decl(&self, decl: &Decl) -> String {
        match decl {
            Decl::Interface(interface) => {
                let fields: String = interface
                    .fields
                    .iter()
                    .map(|f| self.render_field(f))
                    .collect();
                let doc = doc_prefix(interface.doc.as_deref(), "");
                let dep = deprecated_prefix(interface.deprecated.as_deref(), "");
                format!(
                    "{doc}{dep}type {}{} struct {{\n{fields}}}",
                    interface.name.name,
                    type_params(&interface.params)
                )
            }
            Decl::Enum(decl) => self.render_enum(decl),
            Decl::Function(function) => self.render_function(function),
            // A branded well-known type is a named string.
            Decl::Alias(alias) => format!("type {} {}", alias.name.name, alias.value),
            Decl::Raw(raw) => raw.text.clone(),
            Decl::Client(client) => {
                let methods: String = client
                    .methods
                    .iter()
                    .map(|m| self.render_method(m))
                    .collect();
                format!("type {} interface {{\n{methods}}}", client.name.name)
            }
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
            deprecated: None,
            doc: None,
        }
    }

    #[test]
    fn imports_render_as_go_import_lines() {
        // Go imports the whole package, so the per-symbol names are ignored. With
        // no module path the flat package name stands alone.
        assert_eq!(
            GoRules::default().render_import("billing::types", "encoding/json", &["json"]),
            "import \"encoding/json\""
        );
        // A module path prefixes an SDK sub-package so the import resolves, but a
        // standard-library import is left verbatim.
        let rules = GoRules {
            go_module: Some("example.com/sdk".into()),
            current: "payments.charges::types".into(),
        };
        assert_eq!(
            rules.render_import(
                "payments.charges::types",
                "payments.common::types",
                &["Money"]
            ),
            "import \"example.com/sdk/payments/common\""
        );
        // The SDK's shared group is a package of its own, under internal/.
        assert_eq!(
            rules.render_import("payments.charges::types", "::internal", &["MustDescriptor"]),
            "import \"example.com/sdk/internal/tono\""
        );
        assert_eq!(
            rules.render_import("payments.charges::types", "encoding/json", &[]),
            "import \"encoding/json\""
        );
        // An external package whose path segment is not a legal identifier (the
        // runtime's http-go) carries its alias explicitly, so the tonohttp.X
        // references resolve without leaning on the package clause.
        assert_eq!(
            rules.render_import(
                "payments.charges::types",
                "github.com/tono-lang/tono/runtimes/http-go",
                &["tonohttp"]
            ),
            "import tonohttp \"github.com/tono-lang/tono/runtimes/http-go\""
        );
    }

    #[test]
    fn a_struct_renders_fields_with_json_tags() {
        let decl = Decl::Interface(Interface {
            name: Symbol::builtin("Charge"),
            params: vec![],
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
            deprecated: None,
            doc: None,
        });
        let out = GoRules::default().render_decl(&decl);
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
    fn a_generic_struct_renders_its_type_parameter_clause_with_the_any_bound() {
        // Go requires a constraint on each parameter; an unconstrained one is `any`.
        let decl = Decl::Interface(Interface {
            name: Symbol::builtin("Page"),
            params: vec!["T".into()],
            fields: vec![field(
                "Items",
                TypeExpr::list(TypeExpr::Ref(Symbol::builtin("T"))),
                false,
                "items",
            )],
            deprecated: None,
            doc: None,
        });
        assert_eq!(
            GoRules::default().render_decl(&decl),
            "type Page[T any] struct {\n\tItems []T `json:\"items\"`\n}"
        );
    }

    #[test]
    fn collections_stay_slices_and_maps_even_when_optional() {
        let decl = Decl::Interface(Interface {
            name: Symbol::builtin("Bag"),
            params: vec![],
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
            deprecated: None,
            doc: None,
        });
        let out = GoRules::default().render_decl(&decl);
        // An optional slice is not a pointer; it stays a slice, with no omitempty.
        assert!(out.contains("\tTags []string `json:\"tags\"`\n"));
        assert!(out.contains("\tMeta map[string]int32 `json:\"meta\"`\n"));
    }

    #[test]
    fn a_field_without_a_wire_override_tags_with_its_name() {
        let decl = Decl::Interface(Interface {
            name: Symbol::builtin("Charge"),
            params: vec![],
            fields: vec![Field {
                name: Symbol::builtin("Id"),
                ty: TypeExpr::Ref(Symbol::builtin("string")),
                nullable: false,
                wire: None,
                deprecated: None,
                doc: None,
            }],
            deprecated: None,
            doc: None,
        });
        assert!(GoRules::default()
            .render_decl(&decl)
            .contains("\tId string `json:\"Id\"`\n"));
    }

    #[test]
    fn an_enum_renders_a_named_string_and_its_constants() {
        let decl = Decl::Enum(EnumDecl {
            name: Symbol::builtin("Status"),
            members: vec![Symbol::builtin("pending"), Symbol::builtin("in_review")],
            member_docs: vec![None, None],
            backing: EnumRepr::String,
            deprecated: None,
            doc: None,
        });
        assert_eq!(
            GoRules::default().render_decl(&decl),
            "type Status string\n\nconst (\n\tStatusPending Status = \"pending\"\n\t\
             StatusInReview Status = \"in_review\"\n)"
        );
    }

    #[test]
    fn an_int_backed_enum_renders_a_named_int_and_its_integer_constants() {
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
            GoRules::default().render_decl(&decl),
            "type HTTPCode int\n\nconst (\n\tHTTPCodeOk HTTPCode = 200\n\t\
             HTTPCodeNotFound HTTPCode = 404\n\tHTTPCodeError HTTPCode = 500\n)"
        );
    }

    #[test]
    fn an_empty_enum_is_just_the_named_string() {
        let decl = Decl::Enum(EnumDecl {
            name: Symbol::builtin("Empty"),
            members: vec![],
            member_docs: vec![],
            backing: EnumRepr::String,
            deprecated: None,
            doc: None,
        });
        assert_eq!(GoRules::default().render_decl(&decl), "type Empty string\n");
    }

    #[test]
    fn a_deprecated_struct_field_and_enum_carry_godoc_comments() {
        let decl = Decl::Interface(Interface {
            name: Symbol::builtin("Charge"),
            params: vec![],
            fields: vec![Field {
                name: Symbol::builtin("Amount"),
                ty: TypeExpr::Ref(Symbol::builtin("uint64")),
                nullable: false,
                wire: Some("amount".into()),
                deprecated: Some("use AmountCents".into()),
                doc: None,
            }],
            deprecated: Some("use ChargeV2".into()),
            doc: None,
        });
        let out = GoRules::default().render_decl(&decl);
        // The godoc line sits directly above the type and the field, no blank line.
        assert!(out.contains("// Deprecated: use ChargeV2\ntype Charge struct"));
        assert!(out.contains("\t// Deprecated: use AmountCents\n\tAmount"));

        // A bare `@deprecated` (no reason) renders the plain marker above the enum.
        let enum_decl = Decl::Enum(EnumDecl {
            name: Symbol::builtin("Status"),
            members: vec![Symbol::builtin("open")],
            member_docs: vec![None],
            backing: EnumRepr::String,
            deprecated: Some(String::new()),
            doc: None,
        });
        assert!(GoRules::default()
            .render_decl(&enum_decl)
            .starts_with("// Deprecated:\ntype Status string"));
    }

    #[test]
    fn doc_renders_as_flattened_godoc_on_the_struct_field_and_enum_member() {
        let decl = Decl::Interface(Interface {
            name: Symbol::builtin("Charge"),
            params: vec![],
            fields: vec![Field {
                name: Symbol::builtin("Amount"),
                ty: TypeExpr::Ref(Symbol::builtin("int64")),
                nullable: false,
                wire: Some("amount".into()),
                deprecated: None,
                doc: Some("The **amount** in minor units.".into()),
            }],
            deprecated: None,
            doc: Some("A billing charge.".into()),
        });
        let out = GoRules::default().render_decl(&decl);
        // godoc sits directly above the type and the field; Markdown is flattened.
        assert!(out.starts_with("// A billing charge.\ntype Charge struct"));
        assert!(out.contains("\t// The amount in minor units.\n\tAmount"));

        // A per-member doc renders above its const.
        let enum_decl = Decl::Enum(EnumDecl {
            name: Symbol::builtin("Status"),
            members: vec![Symbol::builtin("open"), Symbol::builtin("closed")],
            member_docs: vec![Some("Still open.".into()), None],
            backing: EnumRepr::String,
            deprecated: None,
            doc: None,
        });
        let out = GoRules::default().render_decl(&enum_decl);
        assert!(out.contains("\t// Still open.\n\tStatusOpen Status = \"open\""));
    }

    #[test]
    fn type_expressions_render_idiomatically() {
        let rules = GoRules::default();
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
            GoRules::default().render_decl(&function),
            "func Decode(data []byte) error {\n\treturn nil\n}"
        );
    }

    #[test]
    fn an_alias_renders_as_a_named_type() {
        let alias = Decl::Alias(Alias {
            name: Symbol::builtin("Uuid"),
            value: "string".into(),
        });
        assert_eq!(GoRules::default().render_decl(&alias), "type Uuid string");
    }

    #[test]
    fn a_raw_item_renders_verbatim() {
        let raw = Decl::Raw(Raw {
            text: "func (m Method) foo() {}".into(),
            refs: vec![],
            ..Raw::default()
        });
        assert_eq!(
            GoRules::default().render_decl(&raw),
            "func (m Method) foo() {}"
        );
    }

    #[test]
    fn union_and_method_arms_render_nothing_here() {
        let union = Decl::Union(UnionDecl {
            name: Symbol::builtin("Method"),
            discriminator: "type".into(),
            variants: vec![],
            deprecated: None,
            doc: None,
        });
        let method = Decl::Method(Method {
            name: Symbol::builtin("Ping"),
            params: vec![],
            ret: None,
            err: None,
            is_async: false,
            doc: None,
        });
        assert_eq!(GoRules::default().render_decl(&union), "");
        assert_eq!(GoRules::default().render_decl(&method), "");
    }

    #[test]
    fn a_client_renders_a_blocking_interface_with_the_error_pair() {
        // Go lowers async and sync operations to the same blocking signature;
        // the error channel is the (T, error) pair, or a bare error with no
        // output.
        let decl = Decl::Client(crate::codegen::tree::ClientDecl {
            name: Symbol::builtin("Client"),
            methods: vec![
                Method {
                    name: Symbol::builtin("CreateCharge"),
                    params: vec![field(
                        "input",
                        TypeExpr::Ref(Symbol::builtin("CreateChargeInput")),
                        false,
                        "input",
                    )],
                    ret: Some(TypeExpr::Ref(Symbol::builtin("Charge"))),
                    err: Some(TypeExpr::Ref(Symbol::builtin("error"))),
                    is_async: true,
                    doc: None,
                },
                Method {
                    name: Symbol::builtin("Ping"),
                    params: vec![],
                    ret: None,
                    err: Some(TypeExpr::Ref(Symbol::builtin("error"))),
                    is_async: false,
                    doc: None,
                },
            ],
        });
        assert_eq!(
            GoRules::default().render_decl(&decl),
            "type Client interface {\n\tCreateCharge(input CreateChargeInput) \
             (Charge, error)\n\tPing() error\n}"
        );
    }
}
