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
                format!("{alias} \"{full}\"")
            }
            _ => format!("\"{full}\""),
        }
    }

    /// Go groups its imports in one parenthesized block, which is what its own
    /// formatter sorts and what a reader expects; a run of loose `import "x"`
    /// lines is valid but gofmt will not fold it, so the shape has to be right
    /// when it is written. A single import needs no block.
    fn render_imports(&self, statements: Vec<String>) -> String {
        match statements.as_slice() {
            [one] => format!("import {one}"),
            many => format!(
                "import (\n{}\n)",
                many.iter()
                    .map(|s| format!("\t{s}"))
                    .collect::<Vec<_>>()
                    .join("\n")
            ),
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
#[path = "render_tests.rs"]
mod tests;
