//! The conversion a value spelled under its own TypeScript type goes
//! through at the boundary. TypeScript is structurally typed, so most
//! spellings are the same type written another way and the value passes as
//! it is, for `tsc` to grade against the library's own declaration. The one
//! divide structure cannot cross is number/bigint: tono's 64-bit integers
//! are `bigint` and the narrower ones `number`, so a spelling on the other
//! side asks for a real conversion, and the emitted call must write it or
//! `tsc` rejects generated code the check accepted. A primitive spelling
//! with no conversion (a `string` asked to cross as `number`) is refused
//! before generation, naming both types.

use super::ts_type;
use crate::ir::{Prim, Tref};

/// The primitive type names a TypeScript spelling can ask a value to cross
/// as. Only these refuse on a mismatch with the default mapping: any other
/// spelling is structural and `tsc` grades it against the library.
const PRIMITIVES: [&str; 4] = ["boolean", "number", "bigint", "string"];

/// The conversion a value of the logical type `t` goes through to cross as
/// `spelling`: an integer crosses the number/bigint divide with
/// `Number(..)`/`BigInt(..)`, the type's own default spelling passes as is,
/// and any non-primitive spelling is structural. `Err` names both types
/// when the spelling is a primitive TypeScript has no conversion for
/// (`BigInt` of a fractional number throws, so a float never converts).
pub(super) fn coerce(t: &Tref, spelling: &str, expr: &str) -> Result<String, String> {
    let default = ts_type(t);
    if spelling == default {
        return Ok(expr.to_string());
    }
    if let Tref::Prim(p) = t {
        let integer = matches!(
            p,
            Prim::I8
                | Prim::I16
                | Prim::I32
                | Prim::I64
                | Prim::U8
                | Prim::U16
                | Prim::U32
                | Prim::U64
        );
        if integer && default == "bigint" && spelling == "number" {
            return Ok(format!("Number({expr})"));
        }
        if integer && default == "number" && spelling == "bigint" {
            return Ok(format!("BigInt({expr})"));
        }
    }
    if PRIMITIVES.contains(&spelling) {
        return Err(format!(
            "cannot pass a {default} as {spelling} in TypeScript: no conversion from {default} to {spelling}"
        ));
    }
    Ok(expr.to_string())
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
