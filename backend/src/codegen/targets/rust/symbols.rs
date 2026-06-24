//! The Rust Symbol table: maps an IR type reference to its Rust symbol.

use crate::codegen::conventions::leaf_symbol_of;
use crate::codegen::symbol::Symbol;
use crate::ir::{Prim, Tref};

/// Map an IR type reference to the Rust symbol that represents it. Rust spells its
/// structural collections `Vec<T>` and `HashMap<K, V>`; everything else is shared
/// dispatch.
pub fn symbol_of(t: &Tref) -> Symbol {
    leaf_symbol_of(t, prim_symbol, "Vec", "HashMap")
}

/// The Rust representation of a primitive. Integers map to their exact-width Rust
/// type both signs (64-bit included: Rust holds `i64`/`u64` natively and the
/// string-on-wire encoding is a codec concern). `float` is `f64`, `bytes` is
/// `Vec<u8>`, and the well-known types are branded newtypes named for their kind.
fn prim_symbol(p: &Prim) -> Symbol {
    let name = match p {
        Prim::Bool => "bool",
        Prim::String => "String",
        Prim::Bytes => "Vec<u8>",
        Prim::I8 => "i8",
        Prim::I16 => "i16",
        Prim::I32 => "i32",
        Prim::I64 => "i64",
        Prim::U8 => "u8",
        Prim::U16 => "u16",
        Prim::U32 => "u32",
        Prim::U64 => "u64",
        Prim::Float => "f64",
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
    fn primitives_map_to_their_rust_types() {
        assert_prim_symbols(
            symbol_of,
            &[
                (Prim::Bool, "bool"),
                (Prim::String, "String"),
                (Prim::Bytes, "Vec<u8>"),
                (Prim::I8, "i8"),
                (Prim::I16, "i16"),
                (Prim::I32, "i32"),
                (Prim::I64, "i64"),
                (Prim::U8, "u8"),
                (Prim::U16, "u16"),
                (Prim::U32, "u32"),
                (Prim::U64, "u64"),
                (Prim::Float, "f64"),
                (Prim::Timestamp, "Timestamp"),
                (Prim::Date, "LocalDate"),
                (Prim::Duration, "Duration"),
                (Prim::Uuid, "Uuid"),
            ],
        );
    }

    #[test]
    fn sixty_four_bit_ints_stay_native_both_signs() {
        // Rust holds i64/u64 natively; the string-on-wire form is a codec concern.
        assert_eq!(symbol_of(&Tref::Prim(Prim::I64)).name, "i64");
        assert_eq!(symbol_of(&Tref::Prim(Prim::U64)).name, "u64");
    }

    #[test]
    fn param_and_collection_fallbacks() {
        assert_param_and_collections(symbol_of, "Vec", "HashMap");
    }
}
