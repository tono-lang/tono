//! The Rust Symbol table: maps an IR type reference to its Rust symbol.

use crate::codegen::conventions::{leaf_symbol_of, prim_spelling};
use crate::codegen::symbol::Symbol;
use crate::ir::{Prim, Tref};

/// Map an IR type reference to the Rust symbol that represents it. Rust spells its
/// structural collections `Vec<T>` and `HashMap<K, V>`; everything else is shared
/// dispatch.
pub fn symbol_of(t: &Tref) -> Symbol {
    leaf_symbol_of(t, prim_symbol, "Vec", "HashMap")
}

/// The Rust spelling of a primitive (the `rust` column of the shared table).
fn prim_symbol(p: &Prim) -> Symbol {
    Symbol::builtin(prim_spelling(p).rust)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codegen::test_support::{assert_param_and_collections, assert_prim_symbols};

    #[test]
    fn primitives_map_to_their_rust_types() {
        // The full prim table is exercised in the conventions tests; here we only
        // confirm `symbol_of` reads the Rust column, including the native 64-bit
        // ints (the string-on-wire form is a codec concern), bytes, and a branded
        // well-known type.
        assert_prim_symbols(
            symbol_of,
            &[
                (Prim::Bool, "bool"),
                (Prim::Bytes, "Vec<u8>"),
                (Prim::I64, "i64"),
                (Prim::U64, "u64"),
                (Prim::Timestamp, "Timestamp"),
            ],
        );
    }

    #[test]
    fn param_and_collection_fallbacks() {
        assert_param_and_collections(symbol_of, "Vec", "HashMap");
    }
}
