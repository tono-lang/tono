//! The Go Symbol table: maps an IR type reference to its Go symbol.

use crate::codegen::symbol::Symbol;
use crate::ir::{Prim, Tref};

/// Map an IR type reference to the Go symbol that represents it.
///
/// Collections are structural in Go (`[]T`, `map[K]V`), so they have no single
/// nominal symbol; a target builds those as type expressions and only reaches
/// `symbol_of` for the leaf types. The fallbacks here keep the function total
/// when a collection reference is passed directly.
pub fn symbol_of(t: &Tref) -> Symbol {
    match t {
        Tref::Prim(p) => prim_symbol(p),
        Tref::Param(name) => Symbol::builtin(name.clone()),
        Tref::Ref { id, .. } => ref_symbol(id),
        Tref::List(_) => Symbol::builtin("[]"),
        Tref::Map(_, _) => Symbol::builtin("map"),
    }
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

/// A nominal reference `module#Name` becomes a symbol imported from `module`; an
/// id without a module separator is treated as an in-scope name. (Cross-package
/// qualification is a later, multi-package concern; the in-code name is the bare
/// type, which is correct within a single package.)
fn ref_symbol(id: &str) -> Symbol {
    match id.split_once('#') {
        Some((module, name)) => Symbol::imported(name, module, name),
        None => Symbol::builtin(id),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codegen::symbol::Import;

    #[test]
    fn primitives_map_to_their_go_types() {
        let cases = [
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
        ];
        for (prim, expected) in cases {
            let symbol = symbol_of(&Tref::Prim(prim.clone()));
            assert_eq!(symbol.name, expected, "{prim:?}");
            assert_eq!(
                symbol.import, None,
                "primitives are not imported ({prim:?})"
            );
        }
    }

    #[test]
    fn sixty_four_bit_ints_stay_native_both_signs() {
        // Go holds int64/uint64 natively; the string-on-wire form is a tag option.
        assert_eq!(symbol_of(&Tref::Prim(Prim::I64)).name, "int64");
        assert_eq!(symbol_of(&Tref::Prim(Prim::U64)).name, "uint64");
    }

    #[test]
    fn a_nominal_ref_is_imported_from_its_module() {
        let symbol = symbol_of(&Tref::Ref {
            id: "payments#Charge".into(),
            args: vec![],
        });
        assert_eq!(symbol.name, "Charge");
        assert_eq!(
            symbol.import,
            Some(Import {
                module: "payments".into(),
                imported: "Charge".into(),
            })
        );
    }

    #[test]
    fn a_ref_without_a_module_is_an_in_scope_name() {
        let symbol = symbol_of(&Tref::Ref {
            id: "Bare".into(),
            args: vec![],
        });
        assert_eq!(symbol.name, "Bare");
        assert_eq!(symbol.import, None);
    }

    #[test]
    fn a_type_param_is_a_local_name() {
        let symbol = symbol_of(&Tref::Param("T".into()));
        assert_eq!(symbol.name, "T");
        assert_eq!(symbol.import, None);
    }

    #[test]
    fn collections_have_structural_fallback_symbols() {
        assert_eq!(
            symbol_of(&Tref::List(Box::new(Tref::Prim(Prim::Bool)))).name,
            "[]"
        );
        assert_eq!(
            symbol_of(&Tref::Map(
                Box::new(Tref::Prim(Prim::String)),
                Box::new(Tref::Prim(Prim::Bool)),
            ))
            .name,
            "map"
        );
    }
}
