//! The shared recursion for rendering a type expression into a target's surface
//! syntax.
//!
//! A target supplies only how it *spells* each composite construct — a list, a
//! map, a nullable, a generic application, an `@entries` pairs-array — as a few
//! one-line formatters. The walk over nested type expressions lives here once, so
//! adding a language means writing those formatters, not re-deriving the
//! recursion.

use crate::codegen::tree::TypeExpr;

/// How a target spells each composite type construct. A leaf reference renders as
/// its symbol name, so only the composites need a method.
pub trait TypeSyntax {
    /// `Vec<inner>` / `[]inner` / `inner[]`.
    fn list(&self, inner: &str) -> String;
    /// `HashMap<k, v>` / `map[k]v` / `Record<k, v>`.
    fn map(&self, key: &str, value: &str) -> String;
    /// `Option<inner>` / `*inner` / `inner | null`.
    fn nullable(&self, inner: &str) -> String;
    /// `Name<args>` / `Name[args]`.
    fn generic(&self, name: &str, args: &[String]) -> String;
    /// The `@entries` pairs-array: `Vec<(k, v)>` / `[]Entry[k, v]` / `[k, v][]`.
    fn entries(&self, key: &str, value: &str) -> String;
}

/// Render a type expression into surface syntax, recursing through nested
/// expressions and delegating each construct's spelling to `syntax`.
pub fn render_type(ty: &TypeExpr, syntax: &impl TypeSyntax) -> String {
    match ty {
        TypeExpr::Ref(symbol) => symbol.name.clone(),
        TypeExpr::List(inner) => syntax.list(&render_type(inner, syntax)),
        TypeExpr::Map(key, value) => {
            syntax.map(&render_type(key, syntax), &render_type(value, syntax))
        }
        TypeExpr::Nullable(inner) => syntax.nullable(&render_type(inner, syntax)),
        TypeExpr::Generic(symbol, args) => {
            let rendered: Vec<String> = args.iter().map(|a| render_type(a, syntax)).collect();
            syntax.generic(&symbol.name, &rendered)
        }
        TypeExpr::Entries(key, value) => {
            syntax.entries(&render_type(key, syntax), &render_type(value, syntax))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codegen::symbol::Symbol;

    // A stand-in syntax to exercise the recursion independent of any target.
    struct Angle;
    impl TypeSyntax for Angle {
        fn list(&self, inner: &str) -> String {
            format!("List<{inner}>")
        }
        fn map(&self, key: &str, value: &str) -> String {
            format!("Map<{key}, {value}>")
        }
        fn nullable(&self, inner: &str) -> String {
            format!("Opt<{inner}>")
        }
        fn generic(&self, name: &str, args: &[String]) -> String {
            format!("{name}<{}>", args.join(", "))
        }
        fn entries(&self, key: &str, value: &str) -> String {
            format!("Pairs<{key}, {value}>")
        }
    }

    #[test]
    fn the_recursion_descends_every_construct() {
        let r = |n: &str| TypeExpr::Ref(Symbol::builtin(n));
        assert_eq!(render_type(&r("X"), &Angle), "X");
        assert_eq!(render_type(&TypeExpr::list(r("X")), &Angle), "List<X>");
        assert_eq!(
            render_type(&TypeExpr::map(r("K"), r("V")), &Angle),
            "Map<K, V>"
        );
        assert_eq!(render_type(&TypeExpr::nullable(r("X")), &Angle), "Opt<X>");
        assert_eq!(
            render_type(
                &TypeExpr::Generic(Symbol::builtin("Page"), vec![r("X")]),
                &Angle
            ),
            "Page<X>"
        );
        assert_eq!(
            render_type(&TypeExpr::entries(r("K"), r("V")), &Angle),
            "Pairs<K, V>"
        );
        // Nesting composes through the shared walk.
        assert_eq!(
            render_type(&TypeExpr::list(TypeExpr::nullable(r("X"))), &Angle),
            "List<Opt<X>>"
        );
    }
}
