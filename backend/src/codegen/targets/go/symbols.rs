//! The Go Symbol table: maps an IR type reference to its Go symbol.

use crate::codegen::conventions::leaf_symbol_of;
use crate::codegen::symbol::Symbol;
use crate::ir::{Prim, Tref};

/// Map an IR type reference to the Go symbol that represents it. Go spells its
/// structural collections `[]T` and `map[K]V`; everything else is shared dispatch.
pub fn symbol_of(t: &Tref) -> Symbol {
    leaf_symbol_of(t, prim_symbol, "[]", "map")
}

/// The Go representation of a primitive. Integers map to their exact-width Go
/// type both signs; 64-bit integers stay native (`int64`/`uint64`) and the
/// string-on-wire encoding rides the json `,string` tag option. `bytes` is
/// `[]byte`, which `encoding/json` base64-encodes automatically. The well-known
/// types are named string wrappers.
fn prim_symbol(p: &Prim) -> Symbol {
    let name = match p {
        Prim::Bool => "bool",
        Prim::String => "string",
        Prim::Bytes => "[]byte",
        Prim::I8 => "int8",
        Prim::I16 => "int16",
        Prim::I32 => "int32",
        Prim::I64 => "int64",
        Prim::U8 => "uint8",
        Prim::U16 => "uint16",
        Prim::U32 => "uint32",
        Prim::U64 => "uint64",
        Prim::Float => "float64",
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
    fn primitives_map_to_their_go_types() {
        assert_prim_symbols(
            symbol_of,
            &[
                (Prim::Bool, "bool"),
                (Prim::String, "string"),
                (Prim::Bytes, "[]byte"),
                (Prim::I8, "int8"),
                (Prim::I16, "int16"),
                (Prim::I32, "int32"),
                (Prim::I64, "int64"),
                (Prim::U8, "uint8"),
                (Prim::U16, "uint16"),
                (Prim::U32, "uint32"),
                (Prim::U64, "uint64"),
                (Prim::Float, "float64"),
                (Prim::Timestamp, "Timestamp"),
                (Prim::Date, "LocalDate"),
                (Prim::Duration, "Duration"),
                (Prim::Uuid, "Uuid"),
            ],
        );
    }

    #[test]
    fn sixty_four_bit_ints_stay_native_both_signs() {
        // Go holds int64/uint64 natively; the string-on-wire form is a tag option.
        assert_eq!(symbol_of(&Tref::Prim(Prim::I64)).name, "int64");
        assert_eq!(symbol_of(&Tref::Prim(Prim::U64)).name, "uint64");
    }

    #[test]
    fn param_and_collection_fallbacks() {
        assert_param_and_collections(symbol_of, "[]", "map");
    }
}
