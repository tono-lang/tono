//! The conversion a value spelled under its own TypeScript type goes
//! through at the boundary. TypeScript is structurally typed, so most
//! spellings are the same type written another way and the value passes as
//! it is, for `tsc` to grade against the library's own declaration. The one
//! divide structure cannot cross is number/bigint: tono's 64-bit integers
//! are `bigint` and the narrower ones `number`, so a spelling on the other
//! side asks for a real conversion, and the emitted call must write it or
//! `tsc` rejects generated code the check accepted. The other divide is the
//! keyed collection: a generated map type is a plain object (`Record`),
//! while a library keeps a table in a `Map`, and neither is assignable to
//! the other. A primitive spelling with no conversion (a `string` asked to
//! cross as `number`) is refused before generation, naming both types. The
//! same divides run the other way when a `yields` position is spelled:
//! what the library answers converts back into the declared type.

use super::ts_type;
use crate::ir::{Prim, Tref};

/// The primitive type names a TypeScript spelling can ask a value to cross
/// as. Only these refuse on a mismatch with the default mapping: any other
/// spelling is structural and `tsc` grades it against the library.
const PRIMITIVES: [&str; 4] = ["boolean", "number", "bigint", "string"];

/// Whether a spelling names the keyed collection class (`Map<..>`,
/// `ReadonlyMap<..>`), the shape a library keeps a table in, where the
/// generated map type is a plain object.
fn map_class(spelling: &str) -> bool {
    let s = spelling.trim_start();
    s.starts_with("Map<") || s.starts_with("ReadonlyMap<")
}

fn is_integer(t: &Tref) -> bool {
    matches!(
        t,
        Tref::Prim(
            Prim::I8
                | Prim::I16
                | Prim::I32
                | Prim::I64
                | Prim::U8
                | Prim::U16
                | Prim::U32
                | Prim::U64
        )
    )
}

/// Which way a value crosses the boundary: into the spelling a binding
/// declares (an argument, [`coerce`]), or back from it (a spelled answer,
/// [`coerce_back`]). The divides are the same either way; only the
/// expression written and the side named first in a refusal change.
#[derive(Clone, Copy)]
enum Way {
    Into,
    Back,
}

/// The one conversion cascade, run either way: the type's own default
/// spelling passes as is; an integer crosses the number/bigint divide with
/// `Number(..)`/`BigInt(..)`; a map crosses between the plain object a
/// generated map is and the `Map` class a library keeps a table in; any
/// other non-primitive spelling is structural, for `tsc` to grade. `Err`
/// names both types when the spelling is a primitive TypeScript has no
/// conversion for (`BigInt` of a fractional number throws, so a float
/// never converts).
fn convert(t: &Tref, spelling: &str, expr: &str, way: Way) -> Result<String, String> {
    let default = ts_type(t);
    if spelling == default {
        return Ok(expr.to_string());
    }
    // The value is on the `default` side going in and on the `spelling`
    // side coming back, so the divide reads the other way round.
    let (from, to) = match way {
        Way::Into => (default.as_str(), spelling),
        Way::Back => (spelling, default.as_str()),
    };
    if is_integer(t) && from == "bigint" && to == "number" {
        return Ok(format!("Number({expr})"));
    }
    if is_integer(t) && from == "number" && to == "bigint" {
        return Ok(format!("BigInt({expr})"));
    }
    if matches!(t, Tref::Map(_, _)) && map_class(spelling) {
        return Ok(match way {
            Way::Into => format!("new Map(Object.entries({expr}))"),
            Way::Back => format!("Object.fromEntries({expr})"),
        });
    }
    if PRIMITIVES.contains(&spelling) {
        let verb = match way {
            Way::Into => "pass",
            Way::Back => "read",
        };
        return Err(format!(
            "cannot {verb} a {from} as {to} in TypeScript: no conversion from {from} to {to}"
        ));
    }
    Ok(expr.to_string())
}

/// The conversion a value of the logical type `t` goes through to cross as
/// `spelling` (see [`convert`]).
pub(super) fn coerce(t: &Tref, spelling: &str, expr: &str) -> Result<String, String> {
    convert(t, spelling, expr, Way::Into)
}

/// The conversion a value the library answered under `spelling` goes
/// through to become the logical type `t`: [`coerce`] run the other way.
pub(super) fn coerce_back(t: &Tref, spelling: &str, expr: &str) -> Result<String, String> {
    convert(t, spelling, expr, Way::Back)
}

/// The conversion a struct literal of the form `form_type` goes through to
/// cross as `spelling`: an object literal is structural, so any
/// non-primitive spelling passes it as it is for `tsc` to grade against the
/// library (TypeScript has no pointer to take; `&Options` is not a type
/// here and the compiler says so). A primitive spelling names a type no
/// object literal can cross as, and is refused naming both.
pub(super) fn form_coerce(form_type: &str, spelling: &str, expr: &str) -> Result<String, String> {
    if PRIMITIVES.contains(&spelling) {
        return Err(format!(
            "cannot pass a {form_type} literal as {spelling} in TypeScript: no conversion from {form_type} to {spelling}"
        ));
    }
    Ok(expr.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn prim(p: Prim) -> Tref {
        Tref::Prim(p)
    }

    #[test]
    fn an_integer_crosses_the_bigint_divide_in_both_directions() {
        assert_eq!(
            coerce(&prim(Prim::I64), "number", "s.port").unwrap(),
            "Number(s.port)"
        );
        assert_eq!(
            coerce(&prim(Prim::U64), "number", "v").unwrap(),
            "Number(v)"
        );
        assert_eq!(
            coerce(&prim(Prim::U32), "bigint", "v").unwrap(),
            "BigInt(v)"
        );
    }

    #[test]
    fn the_default_spelling_passes_as_is() {
        assert_eq!(coerce(&prim(Prim::I64), "bigint", "v").unwrap(), "v");
        assert_eq!(coerce(&prim(Prim::String), "string", "v").unwrap(), "v");
        assert_eq!(
            coerce(
                &Tref::List(Box::new(prim(Prim::Float))),
                "number[]",
                "values"
            )
            .unwrap(),
            "values"
        );
    }

    #[test]
    fn a_structural_spelling_passes_for_the_compiler_to_grade() {
        assert_eq!(
            coerce(
                &Tref::List(Box::new(prim(Prim::Float))),
                "Array<number>",
                "values"
            )
            .unwrap(),
            "values"
        );
        assert_eq!(
            coerce(&prim(Prim::String), "`a-${string}`", "v").unwrap(),
            "v"
        );
    }

    /// An object literal is structural: any type name passes it for the
    /// compiler to grade, and only a primitive is refused.
    #[test]
    fn a_form_literal_passes_structurally_and_refuses_a_primitive() {
        assert_eq!(
            form_coerce("Options", "Options", "{ a: 1 }").unwrap(),
            "{ a: 1 }"
        );
        assert_eq!(
            form_coerce("Options", "Partial<Options>", "{ a: 1 }").unwrap(),
            "{ a: 1 }"
        );
        let err = form_coerce("Options", "string", "{ a: 1 }").unwrap_err();
        assert!(
            err.contains("no conversion from Options to string"),
            "{err}"
        );
    }

    /// A generated map is a plain object; a library's table is a `Map`.
    /// The spelling asks for the crossing and the call writes it, in
    /// either direction; the default spelling passes as is.
    #[test]
    fn a_map_crosses_into_a_map_class_and_back() {
        let table = Tref::Map(Box::new(prim(Prim::String)), Box::new(prim(Prim::String)));
        assert_eq!(
            coerce(&table, "Map<string, string>", "mappings").unwrap(),
            "new Map(Object.entries(mappings))"
        );
        assert_eq!(
            coerce(&table, "ReadonlyMap<string, string>", "mappings").unwrap(),
            "new Map(Object.entries(mappings))"
        );
        assert_eq!(
            coerce(&table, "Record<string, string>", "mappings").unwrap(),
            "mappings"
        );
        assert_eq!(
            coerce_back(&table, "Map<string, string>", "raw").unwrap(),
            "Object.fromEntries(raw)"
        );
        assert_eq!(
            coerce_back(&table, "Record<string, string>", "raw").unwrap(),
            "raw"
        );
    }

    /// The way back runs the same divides in reverse and refuses the same
    /// primitives, naming both types the other way round.
    #[test]
    fn a_value_answered_under_a_spelling_converts_back() {
        assert_eq!(
            coerce_back(&prim(Prim::I64), "number", "raw").unwrap(),
            "BigInt(raw)"
        );
        assert_eq!(
            coerce_back(&prim(Prim::U32), "bigint", "raw").unwrap(),
            "Number(raw)"
        );
        assert_eq!(
            coerce_back(&prim(Prim::I64), "bigint", "raw").unwrap(),
            "raw"
        );
        assert_eq!(
            coerce_back(&prim(Prim::String), "`a-${string}`", "raw").unwrap(),
            "raw"
        );
        let err = coerce_back(&prim(Prim::String), "number", "raw").unwrap_err();
        assert_eq!(
            err,
            "cannot read a number as string in TypeScript: no conversion from number to string"
        );
    }

    #[test]
    fn a_primitive_with_no_conversion_is_refused_naming_both_types() {
        let err = coerce(&prim(Prim::String), "number", "v").unwrap_err();
        assert_eq!(
            err,
            "cannot pass a string as number in TypeScript: no conversion from string to number"
        );
        // `BigInt` of a fractional value throws at runtime: a float never
        // converts, even though its default spelling is `number`.
        let err = coerce(&prim(Prim::Float), "bigint", "v").unwrap_err();
        assert!(err.contains("no conversion from number to bigint"), "{err}");
        let err = coerce(&prim(Prim::Bytes), "string", "v").unwrap_err();
        assert!(
            err.contains("no conversion from Uint8Array to string"),
            "{err}"
        );
    }
}
