//! The SDK-root shared packages: the emitted transport, the record encoder,
//! and the casing transforms an `@str::` pipeline lowers to. Every entry of
//! every module calls into these rather than each carrying its own copy, so a
//! reference to one of these names is a slot the caller declares (see
//! [`shared_symbol`]/[`shared_slot`]) instead of a direct call.

use super::*;

/// The SDK-root groups this target emits, each named for what it holds, with
/// every optional transport piece on. Used for name-to-group resolution (see
/// [`shared_group`]), where only which group declares a name matters;
/// emission goes through [`shared_groups_for`], which prunes the transport to
/// what the model uses.
pub fn shared_groups() -> Vec<(&'static str, Vec<Decl>)> {
    shared_groups_with(&send::Usage::all())
}

/// The SDK-root groups shaped by what the model actually uses: a model with no
/// `@retry`, `@timeout`, or request hook anywhere gets a transport without
/// those pieces.
pub fn shared_groups_for(model: &crate::ir::Model) -> Vec<(&'static str, Vec<Decl>)> {
    shared_groups_with(&send::usage_of(model))
}

fn shared_groups_with(usage: &send::Usage) -> Vec<(&'static str, Vec<Decl>)> {
    vec![
        ("transport", send::internal_helpers(usage)),
        ("record", vec![record_decl()]),
        ("casing", casing_decls()),
    ]
}

/// Which SDK-root group declares a shared helper. Read off the emitters rather
/// than listed here, so the answer cannot drift from where the declaration
/// actually lands.
fn shared_group(name: &str) -> String {
    shared_groups()
        .into_iter()
        .find(|(_, decls)| {
            crate::codegen::imports::declared_symbols(decls)
                .iter()
                .any(|declared| declared == name)
        })
        .map(|(group, _)| crate::codegen::group::Group::root(group).path())
        .unwrap_or_default()
}

/// A reference to a name in one of the SDK's shared packages, so the import is
/// collected wherever the raw text calls it.
pub(super) fn shared_symbol(name: &str) -> Symbol {
    Symbol::imported(name, shared_group(name), name)
}

/// A shared helper named inside opaque text: a slot, so the package selector is
/// applied (or dropped) when the file is rendered.
pub(super) fn shared_slot(name: &str) -> String {
    crate::codegen::tree::symbol_slot(name)
}

/// Turning a typed input into the wire record the request positions read
/// individual members from. The input is encoded once; the split that
/// follows only slices that encoding up by member name (never decoding a
/// member's own value), so a value reaches the wire with the exact spelling
/// and precision its own encoder gave it.
fn record_decl() -> Decl {
    Decl::raw_providing(
        "EncodeRecord",
        "// EncodeRecord turns a typed input into the wire record the request\n\
         // positions bind from, through the type's own JSON tags. The input is\n\
         // encoded once; the split that follows only slices that encoding up by\n\
         // member name, never decoding a member's own value.\n\
         func EncodeRecord(v any) (map[string]json.RawMessage, error) {\n\
         \tb, err := json.Marshal(v)\n\
         \tif err != nil {\n\t\treturn nil, err\n\t}\n\
         \tvar record map[string]json.RawMessage\n\
         \tif err := json.Unmarshal(b, &record); err != nil {\n\t\treturn nil, err\n\t}\n\
         \treturn record, nil\n\
         }"
        .to_string(),
        vec![import("json", "encoding/json")],
    )
}

/// The casing transforms an `@str::` pipeline lowers to. They serve every entry
/// of every module, so they live in the SDK's shared package rather than beside
/// any one of them.
fn casing_decls() -> Vec<Decl> {
    vec![
        Decl::raw_providing(
            "StrTransformWords",
            "// StrTransformWords splits a resolved value for the casing transforms:\n\
             // runs of spaces, hyphens, and underscores separate words.\n\
             func StrTransformWords(s string) []string {\n\
             \treturn strings.FieldsFunc(s, func(r rune) bool { return r == ' ' || r == '-' || r == '_' })\n\
             }"
            .to_string(),
            vec![import("strings", "strings")],
        ),
        casing_helper("StrUpperSnake", "\tws := StrTransformWords(s)\n\tfor i := range ws {\n\t\tws[i] = strings.ToUpper(ws[i])\n\t}\n\treturn strings.Join(ws, \"_\")"),
        casing_helper("StrSnake", "\tws := StrTransformWords(s)\n\tfor i := range ws {\n\t\tws[i] = strings.ToLower(ws[i])\n\t}\n\treturn strings.Join(ws, \"_\")"),
        casing_helper("StrKebab", "\tws := StrTransformWords(s)\n\tfor i := range ws {\n\t\tws[i] = strings.ToLower(ws[i])\n\t}\n\treturn strings.Join(ws, \"-\")"),
        casing_helper("StrPascal", "\tws := StrTransformWords(s)\n\tfor i := range ws {\n\t\tif ws[i] != \"\" {\n\t\t\tws[i] = strings.ToUpper(ws[i][:1]) + strings.ToLower(ws[i][1:])\n\t\t}\n\t}\n\treturn strings.Join(ws, \"\")"),
    ]
}

fn casing_helper(name: &str, body: &str) -> Decl {
    Decl::raw_providing(
        name,
        format!("func {name}(s string) string {{\n{body}\n}}"),
        vec![import("strings", "strings")],
    )
}

/// The transform-application expression, innermost first in declared order.
/// Only the language-specific `trim`/`lower`/`upper` spellings are Go's; the
/// shared pipeline folds them and the case-fold helpers.
/// Fold the `@str::*` pipeline, declaring a reference for every shared helper it
/// reaches: the helper lives in the SDK's shared package, so without the
/// reference the call would render with no import behind it.
pub(super) fn apply_transforms(
    expr: String,
    transforms: &[String],
    helpers: &mut Helpers,
    refs: &mut Vec<Symbol>,
) -> String {
    // The fold names each helper it reaches; recording them here is what keeps
    // the reference exact, so an entry that upper-snakes a value does not pull
    // the other three casing helpers behind it.
    let reached = std::cell::RefCell::new(Vec::new());
    let out = crate::codegen::entries::plan::apply_transforms(
        expr,
        transforms,
        &mut helpers.transforms,
        |t, out| match t {
            "trim" => Some(format!("strings.TrimSpace({out})")),
            "lower" => Some(format!("strings.ToLower({out})")),
            "upper" => Some(format!("strings.ToUpper({out})")),
            _ => None,
        },
        |name| {
            reached.borrow_mut().push(name.to_string());
            shared_slot(name)
        },
    );
    for name in reached.into_inner() {
        refs.push(shared_symbol(&name));
    }
    out
}
