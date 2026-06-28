//! The TypeScript Symbol table: maps an IR type reference to its TS symbol.

use crate::codegen::conventions::{leaf_symbol_of, prim_spelling};
use crate::codegen::symbol::Symbol;
use crate::ir::{Prim, Tref};

/// Map an IR type reference to the TypeScript symbol that represents it. TS spells
/// its structural collections `T[]` and `Record<string, V>`; everything else is
/// shared dispatch.
pub fn symbol_of(t: &Tref) -> Symbol {
    leaf_symbol_of(t, prim_symbol, "Array", "Record")
}

/// The TypeScript spelling of a primitive (the `typescript` column of the shared
/// table).
fn prim_symbol(p: &Prim) -> Symbol {
    Symbol::builtin(prim_spelling(p).typescript)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codegen::test_support::{assert_param_and_collections, assert_prim_symbols};

    #[test]
    fn primitives_map_to_their_ts_types() {
        // The full prim table is exercised in the conventions tests; here we only
        // confirm `symbol_of` reads the TypeScript column: a narrow int and float
        // are `number`, the 64-bit ints are `bigint` (precision past 2^53 rides the
        // wire as a string), bytes is `Uint8Array`, and a well-known type is
        // branded.
        assert_prim_symbols(
            symbol_of,
            &[
                (Prim::I32, "number"),
                (Prim::Float, "number"),
                (Prim::I64, "bigint"),
                (Prim::U64, "bigint"),
                (Prim::Bytes, "Uint8Array"),
                (Prim::Timestamp, "Timestamp"),
            ],
        );
    }

    #[test]
    fn param_and_collection_fallbacks() {
        assert_param_and_collections(symbol_of, "Array", "Record");
    }
}
