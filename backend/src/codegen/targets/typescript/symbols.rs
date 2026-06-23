//! The TypeScript Symbol table: maps an IR type reference to its TS symbol.

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

/// A nominal reference `module#Name` becomes a symbol imported from `module`; an
/// id without a module separator is treated as an in-scope name.
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
    fn primitives_map_to_their_ts_types() {
        let cases = [
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
    fn sixty_four_bit_ints_are_bigint_both_signs() {
        assert_eq!(symbol_of(&Tref::Prim(Prim::I64)).name, "bigint");
        assert_eq!(symbol_of(&Tref::Prim(Prim::U64)).name, "bigint");
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
            "Array"
        );
        assert_eq!(
            symbol_of(&Tref::Map(
                Box::new(Tref::Prim(Prim::String)),
                Box::new(Tref::Prim(Prim::Bool)),
            ))
            .name,
            "Record"
        );
    }
}
