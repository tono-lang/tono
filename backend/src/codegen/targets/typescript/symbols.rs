//! The TypeScript Symbol table: maps an IR type reference to its TS symbol.

use crate::codegen::conventions::ref_symbol;
use crate::codegen::symbol::Symbol;
use crate::ir::{Prim, Tref};

/// Map an IR type reference to the TypeScript symbol that represents it.
///
/// Collections are structural in TS (`T[]`, `Record<string, V>`) so they have no
/// single nominal symbol; a target builds those as type expressions and only
/// reaches `symbol_of` for the leaf types. The fallbacks here keep the function
/// total when a collection reference is passed directly.
pub fn symbol_of(t: &Tref) -> Symbol {
    match t {
        Tref::Prim(p) => prim_symbol(p),
        Tref::Param(name) => Symbol::builtin(name.clone()),
        Tref::Ref { id, .. } => ref_symbol(id),
        Tref::List(_) => Symbol::builtin("Array"),
        Tref::Map(_, _) => Symbol::builtin("Record"),
    }
}

/// The TS representation of a primitive. 64-bit integers are `bigint` (they go on
/// the wire as strings, since JS `number` loses precision above 2^53); narrower
/// integers and `float` are `number`. Well-known types are branded strings named
/// for their kind.
fn prim_symbol(p: &Prim) -> Symbol {
    let name = match p {
        Prim::Bool => "boolean",
        Prim::String => "string",
        Prim::Bytes => "Uint8Array",
        Prim::I8 | Prim::I16 | Prim::I32 | Prim::U8 | Prim::U16 | Prim::U32 | Prim::Float => {
            "number"
        }
        Prim::I64 | Prim::U64 => "bigint",
        Prim::Timestamp => "Timestamp",
        Prim::Date => "LocalDate",
        Prim::Duration => "Duration",
        Prim::Uuid => "Uuid",
    };
    Symbol::builtin(name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codegen::test_support::{assert_param_and_collections, assert_prim_symbols};

    #[test]
    fn primitives_map_to_their_ts_types() {
        assert_prim_symbols(
            symbol_of,
            &[
                (Prim::Bool, "boolean"),
                (Prim::String, "string"),
                (Prim::Bytes, "Uint8Array"),
                (Prim::I8, "number"),
                (Prim::I16, "number"),
                (Prim::I32, "number"),
                (Prim::U8, "number"),
                (Prim::U16, "number"),
                (Prim::U32, "number"),
                (Prim::Float, "number"),
                (Prim::I64, "bigint"),
                (Prim::U64, "bigint"),
                (Prim::Timestamp, "Timestamp"),
                (Prim::Date, "LocalDate"),
                (Prim::Duration, "Duration"),
                (Prim::Uuid, "Uuid"),
            ],
        );
    }

    #[test]
    fn sixty_four_bit_ints_are_bigint_both_signs() {
        assert_eq!(symbol_of(&Tref::Prim(Prim::I64)).name, "bigint");
        assert_eq!(symbol_of(&Tref::Prim(Prim::U64)).name, "bigint");
    }

    #[test]
    fn param_and_collection_fallbacks() {
        assert_param_and_collections(symbol_of, "Array", "Record");
    }
}
