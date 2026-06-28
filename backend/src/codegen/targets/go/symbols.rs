//! The Go Symbol table: maps an IR type reference to its Go symbol.

use crate::codegen::conventions::{leaf_symbol_of, prim_spelling};
use crate::codegen::symbol::Symbol;
use crate::ir::{Prim, Tref};

/// Map an IR type reference to the Go symbol that represents it. Go spells its
/// structural collections `[]T` and `map[K]V`; everything else is shared dispatch.
pub fn symbol_of(t: &Tref) -> Symbol {
    leaf_symbol_of(t, prim_symbol, "[]", "map")
}

/// The Go spelling of a primitive (the `go` column of the shared table).
fn prim_symbol(p: &Prim) -> Symbol {
    Symbol::builtin(prim_spelling(p).go)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codegen::test_support::{assert_param_and_collections, assert_prim_symbols};

    #[test]
    fn primitives_map_to_their_go_types() {
        // The full prim table is exercised in the conventions tests; here we only
        // confirm `symbol_of` reads the Go column, including the native 64-bit ints
        // (the string-on-wire form is a json tag option), bytes, and a branded
        // well-known type.
        assert_prim_symbols(
            symbol_of,
            &[
                (Prim::Bool, "bool"),
                (Prim::Bytes, "[]byte"),
                (Prim::I64, "int64"),
                (Prim::U64, "uint64"),
                (Prim::Timestamp, "Timestamp"),
            ],
        );
    }

    #[test]
    fn param_and_collection_fallbacks() {
        assert_param_and_collections(symbol_of, "[]", "map");
    }
}
