//! A foreign spelling is the text inside `#(...)`: a type, a callee, a
//! sentinel, a field source, written the way the library spells it and
//! emitted verbatim. The one thing every target does to it is bring the
//! library's own identifiers into scope: Go and Rust qualify them with the
//! package selector, TypeScript imports them. This module finds those
//! identifiers; each target decides which ones are the library's by
//! supplying the words that are not (its own builtins and keywords, and the
//! module's generated type names).

/// One identifier inside a spelling, with the context that decides whether
/// it can be the library's: an identifier right after a path separator
/// (`.` or `::`) is a member of whatever came before it, never a head, and
/// one right before `.` in Go is a package selector the author wrote out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ident<'a> {
    pub text: &'a str,
    pub start: usize,
    pub end: usize,
    pub after_sep: bool,
    pub before_dot: bool,
}

fn is_ident_start(c: u8) -> bool {
    c.is_ascii_alphabetic() || c == b'_'
}

fn is_ident_cont(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_'
}

/// Every identifier of `spelling`, in order.
pub fn idents(spelling: &str) -> Vec<Ident<'_>> {
    let bytes = spelling.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if is_ident_start(bytes[i]) && (i == 0 || !is_ident_cont(bytes[i - 1])) {
            let start = i;
            while i < bytes.len() && is_ident_cont(bytes[i]) {
                i += 1;
            }
            let before = spelling[..start].trim_end();
            let after_sep = before.ends_with('.') || before.ends_with("::");
            let after = spelling[i..].trim_start();
            let before_dot = after.starts_with('.') && !after.starts_with("..");
            out.push(Ident {
                text: &spelling[start..i],
                start,
                end: i,
                after_sep,
                before_dot,
            });
        } else {
            i += 1;
        }
    }
    out
}

/// The identifiers the library owns: every path head that `is_local` does
/// not claim (a builtin, a keyword, a generated type of the module). With
/// `selectors`, a head right before `.` is a package selector the author
/// wrote out (`pkg.Type` in Go) and is skipped with what it qualifies;
/// without it, `Type.method` is a member of the library's `Type`
/// (TypeScript), and `Type::method` always is (Rust).
pub fn library_idents<'a>(
    spelling: &'a str,
    is_local: &dyn Fn(&str) -> bool,
    selectors: bool,
) -> Vec<Ident<'a>> {
    idents(spelling)
        .into_iter()
        .filter(|id| !id.after_sep && (!selectors || !id.before_dot) && !is_local(id.text))
        .collect()
}

/// `spelling` with every library identifier prefixed by `prefix` (the
/// package selector plus its separator, `mathkit.` or `mathkit::`).
pub fn qualify(
    spelling: &str,
    prefix: &str,
    is_local: &dyn Fn(&str) -> bool,
    selectors: bool,
) -> String {
    let mut out = String::with_capacity(spelling.len() + 16);
    let mut last = 0;
    for id in library_idents(spelling, is_local, selectors) {
        out.push_str(&spelling[last..id.start]);
        out.push_str(prefix);
        out.push_str(id.text);
        last = id.end;
    }
    out.push_str(&spelling[last..]);
    out
}

/// The distinct library identifiers of `spelling`, in first-seen order:
/// what a TypeScript module imports for it.
pub fn library_names(spelling: &str, is_local: &dyn Fn(&str) -> bool) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for id in library_idents(spelling, is_local, false) {
        if !out.iter().any(|n| n == id.text) {
            out.push(id.text.to_string());
        }
    }
    out
}

/// A callee spelled `new X`: TypeScript's class construction. Returns the
/// spelling after the keyword when it is one.
pub fn constructed(spelling: &str) -> Option<&str> {
    spelling
        .strip_prefix("new ")
        .map(str::trim_start)
        .filter(|rest| !rest.is_empty())
}

/// A Go parameter spelled `...T`: the variadic slot, with its element type.
pub fn variadic(spelling: &str) -> Option<&str> {
    spelling.strip_prefix("...").map(str::trim_start)
}

/// The storage a Rust handle keeps behind a box: `Box<dyn T>` (or `Arc<`,
/// `Rc<`), with the wrapper that boxes a freshly constructed value into it.
pub fn rust_boxed(spelling: &str) -> Option<&'static str> {
    for (prefix, wrap) in [
        ("Box<", "Box::new"),
        ("Arc<", "Arc::new"),
        ("Rc<", "Rc::new"),
    ] {
        if spelling.starts_with(prefix) {
            return Some(wrap);
        }
    }
    None
}

/// A Rust spelling `Option<T>`: the `T` inside, for the `Some(..)` wrap.
pub fn rust_option(spelling: &str) -> Option<&str> {
    spelling
        .strip_prefix("Option<")
        .and_then(|rest| rest.strip_suffix('>'))
        .map(str::trim)
}

/// The Go builtins and predeclared names a spelling can use without the
/// package selector.
pub fn go_builtin(word: &str) -> bool {
    matches!(
        word,
        "bool"
            | "string"
            | "int"
            | "int8"
            | "int16"
            | "int32"
            | "int64"
            | "uint"
            | "uint8"
            | "uint16"
            | "uint32"
            | "uint64"
            | "uintptr"
            | "byte"
            | "rune"
            | "float32"
            | "float64"
            | "complex64"
            | "complex128"
            | "error"
            | "any"
            | "map"
            | "chan"
            | "func"
            | "struct"
            | "interface"
            | "nil"
            | "true"
            | "false"
    )
}

/// The Rust keywords, primitives and prelude names a spelling can use
/// without the crate path.
pub fn rust_builtin(word: &str) -> bool {
    matches!(
        word,
        "dyn"
            | "impl"
            | "mut"
            | "ref"
            | "as"
            | "fn"
            | "for"
            | "where"
            | "static"
            | "self"
            | "Self"
            | "crate"
            | "super"
            | "std"
            | "core"
            | "alloc"
            | "bool"
            | "char"
            | "str"
            | "u8"
            | "u16"
            | "u32"
            | "u64"
            | "u128"
            | "usize"
            | "i8"
            | "i16"
            | "i32"
            | "i64"
            | "i128"
            | "isize"
            | "f32"
            | "f64"
            | "String"
            | "Box"
            | "Vec"
            | "Option"
            | "Result"
            | "Arc"
            | "Rc"
            | "Send"
            | "Sync"
            | "Sized"
            | "Clone"
            | "Copy"
            | "Fn"
            | "FnMut"
            | "FnOnce"
            | "HashMap"
            | "BTreeMap"
            | "Some"
            | "None"
            | "Ok"
            | "Err"
    )
}

/// The TypeScript builtins and keywords a spelling can use without an
/// import.
pub fn ts_builtin(word: &str) -> bool {
    matches!(
        word,
        "new"
            | "typeof"
            | "keyof"
            | "readonly"
            | "number"
            | "string"
            | "boolean"
            | "bigint"
            | "symbol"
            | "void"
            | "never"
            | "unknown"
            | "any"
            | "null"
            | "undefined"
            | "object"
            | "Promise"
            | "Array"
            | "Record"
            | "Map"
            | "Set"
            | "Date"
            | "Error"
            | "Partial"
            | "Required"
            | "Readonly"
            | "Uint8Array"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn none(_: &str) -> bool {
        false
    }

    #[test]
    fn heads_are_found_and_members_are_not() {
        let names: Vec<&str> = idents("Box<dyn Calculator<f64>>")
            .iter()
            .map(|i| i.text)
            .collect();
        assert_eq!(names, ["Box", "dyn", "Calculator", "f64"]);
        let ids = idents("FormulaCalculator::parse");
        assert!(!ids[0].after_sep && ids[1].after_sep);
        let ids = idents("ctx context.Context");
        assert!(ids[1].before_dot && ids[2].after_sep);
        let ids = idents("...Calculator[float64]");
        assert!(!ids[0].before_dot, "a variadic ellipsis is not a selector");
    }

    #[test]
    fn qualify_prefixes_only_the_library_heads() {
        assert_eq!(
            qualify(
                "Box<dyn Calculator<f64>>",
                "mathkit::",
                &rust_builtin,
                false
            ),
            "Box<dyn mathkit::Calculator<f64>>"
        );
        assert_eq!(
            qualify(
                "FormulaCalculator::parse",
                "mathkit::",
                &rust_builtin,
                false
            ),
            "mathkit::FormulaCalculator::parse"
        );
        assert_eq!(
            qualify(
                "*Source[AppSettings]",
                "settingskit.",
                &|w| go_builtin(w) || w == "AppSettings",
                true
            ),
            "*settingskit.Source[AppSettings]"
        );
        assert_eq!(
            qualify("FromConstant[float64]", "mathkit.", &go_builtin, true),
            "mathkit.FromConstant[float64]"
        );
        assert_eq!(
            qualify("other.Thing", "mathkit.", &go_builtin, true),
            "other.Thing",
            "an author-qualified Go selector stays as written"
        );
        assert_eq!(
            qualify("Option<u8>", "mathkit::", &rust_builtin, false),
            "Option<u8>"
        );
    }

    #[test]
    fn library_names_are_what_typescript_imports() {
        assert_eq!(
            library_names("new ConstantCalculator", &ts_builtin),
            ["ConstantCalculator"]
        );
        assert_eq!(
            library_names("FormulaCalculator.parse", &ts_builtin),
            ["FormulaCalculator"]
        );
        assert_eq!(
            library_names("Calculator<number>[]", &ts_builtin),
            ["Calculator"]
        );
        assert!(library_names("number[]", &ts_builtin).is_empty());
        assert_eq!(library_names("A<B, A>", &none), ["A", "B"]);
    }

    #[test]
    fn shape_helpers() {
        assert_eq!(
            constructed("new ConstantCalculator"),
            Some("ConstantCalculator")
        );
        assert_eq!(constructed("newish"), None);
        assert_eq!(
            variadic("...Calculator[float64]"),
            Some("Calculator[float64]")
        );
        assert_eq!(variadic("Calculator"), None);
        assert_eq!(rust_boxed("Box<dyn Calculator<f64>>"), Some("Box::new"));
        assert_eq!(rust_boxed("ConstantCalculator<f64>"), None);
        assert_eq!(rust_option("Option<u8>"), Some("u8"));
        assert_eq!(rust_option("u8"), None);
    }
}
