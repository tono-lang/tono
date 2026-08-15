//! IR builders shared by every `ext`/`extern` (RFC-0023) test fixture: the
//! Go emitter's own unit tests (`ext_tests`, `#[cfg(test)]`) and the
//! `go_ext_roundtrip` integration test (a separate binary, linked against
//! this crate's public API rather than compiled with it). Not `cfg(test)`
//! for that reason: an integration test cannot see anything gated that way,
//! so this stays a small, always-compiled, always-public module rather than
//! source the same builders twice.

use crate::ir::{
    CallArg, EntryField, ErrorBinding, ExtLib, ExternDecl, ExternLang, ExternParam, ForeignStruct,
    LangPath, Member, OpaqueType, Prim, ReturnsLit, Shape, ShapeKind, Source, Tref,
};

pub fn string_t() -> Tref {
    Tref::Prim(Prim::String)
}

pub fn member(name: &str, target: Tref, required: bool) -> Member {
    Member {
        name: name.into(),
        target,
        required,
        default: None,
        constraints: vec![],
        traits: vec![],
    }
}

pub fn structure(id: &str, members: Vec<Member>) -> Shape {
    Shape {
        id: id.into(),
        kind: ShapeKind::Structure {
            params: vec![],
            members,
        },
        traits: vec![],
    }
}

pub fn field(name: &str, target: Tref, sources: Vec<Source>) -> EntryField {
    EntryField {
        name: name.into(),
        target,
        sources,
        format: None,
        transforms: vec![],
        select: None,
        call: None,
        binds: vec![],
        constraints: vec![],
        traits: vec![],
    }
}

pub fn call_ref(path: &[&str]) -> CallArg {
    CallArg::Ref(path.iter().map(|s| (*s).to_string()).collect())
}

pub fn ext_param(name: &str, target: Tref) -> ExternParam {
    ExternParam {
        name: name.into(),
        r#type: target,
    }
}

/// A single-language (`go`) extern declaration: the shape every `extern` in
/// these fixtures shares (one `ExternLang`, no per-language variation), so a
/// call site only spells what actually differs between externs (name,
/// params, symbol, yields, returns, errors) instead of repeating the whole
/// `ExternDecl { .. langs: vec![ExternLang { .. }] }` skeleton each time.
#[allow(clippy::too_many_arguments)]
pub fn go_extern(
    name: &str,
    params: Vec<ExternParam>,
    ret: Tref,
    symbol: &str,
    call_args: Vec<CallArg>,
    yields: Vec<crate::ir::YieldsPos>,
    returns: Option<ReturnsLit>,
    errors: Vec<ErrorBinding>,
) -> ExternDecl {
    ExternDecl {
        name: name.into(),
        params,
        r#return: ret,
        langs: vec![ExternLang {
            lang: "go".into(),
            symbol: symbol.into(),
            call_args,
            yields,
            returns,
            errors,
        }],
    }
}

/// An `ext` block declaring only a Go module path, for the common case
/// (these fixtures never exercise a lib bound for more than one target).
pub fn go_ext_lib(
    name: &str,
    path: &str,
    structs: Vec<ForeignStruct>,
    types: Vec<OpaqueType>,
    externs: Vec<ExternDecl>,
) -> ExtLib {
    ExtLib {
        name: name.into(),
        langs: vec![LangPath {
            lang: "go".into(),
            path: path.into(),
        }],
        structs,
        types,
        externs,
    }
}
