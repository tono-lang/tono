//! The Go probe: one line per binding, typed the way the emitter crosses
//! the boundary, compiled by `go build` inside the consumer's module so the
//! library resolves exactly as the generated SDK's import does.
//!
//! A probe function's parameters carry what the binding says crosses (a
//! parameter's own spelling, else the default Go mapping, a handle's
//! declared storage, the context where a position declares it); the call
//! is assigned to results typed by the target's convention (see
//! `ext::build_call`: an implicit trailing `error` unless a `yields:`
//! position marks it). Nothing is inferred from the library: a binding the
//! probe cannot express in declared terms is listed as skipped, with why.

use std::path::Path;

use super::ext::ext_render::{coerce, literal_of_json};
use super::ext::{binds_ctx, handle_go_type, lib_ident, qualify};
use super::{go_type, go_type_label};
use crate::codegen::verify::{
    parse_go_errors, run_probe, Probe, ProbeRun, RunOutcome, Scratch, SiteKey,
};
use crate::ir::{
    CallArg, ExtLib, ExternDecl, ExternLang, ExternParam, Module, OpaqueType, Prim, Tref,
};

const PROBE_FILE: &str = "probe.go";

/// The Go type a declared position has in the probe, when it can be spelled
/// without the generated SDK: builtins, lists and maps of them, handles by
/// their storage, foreign forms by their name. `Err` says what cannot be
/// spelled: a tono shape (a generated type), a support type, a form with
/// no go block.
fn probe_type(module: &Module, lib: &ExtLib, t: &Tref) -> Result<String, String> {
    match t {
        Tref::Prim(p) => match p {
            Prim::Bool
            | Prim::String
            | Prim::Bytes
            | Prim::I8
            | Prim::I16
            | Prim::I32
            | Prim::I64
            | Prim::U8
            | Prim::U16
            | Prim::U32
            | Prim::U64
            | Prim::Float => Ok(go_type(t)),
            _ => Err(format!(
                "{} is spelled by the generated SDK's support code",
                go_type_label(t)
            )),
        },
        Tref::List(inner) => Ok(format!("[]{}", probe_type(module, lib, inner)?)),
        Tref::Param(p) => Err(format!("{p} is a type parameter")),
        Tref::Map(k, v) => Ok(format!(
            "map[{}]{}",
            probe_type(module, lib, k)?,
            probe_type(module, lib, v)?
        )),
        // A type written inside the ext block resolves by its bare name
        // under the module (`svc#calculator`), a handle field's type outside
        // it under the lib (`mathkit#calculator`): both name this lib's own
        // handle or form.
        Tref::Ref { id, .. } => {
            let name = crate::codegen::entries::local_name(id);
            if let Some(handle) = lib.types.iter().find(|h| h.name == name) {
                return handle_go_type(lib, handle, module, &mut Vec::new())
                    .ok_or_else(|| format!("the handle {name} declares no go block"));
            }
            if let Some(form) = lib.structs.iter().find(|s| s.name == name) {
                let go = form
                    .lang("go")
                    .ok_or_else(|| format!("the struct {name} declares no go block"))?;
                return Ok(qualify(&go.name, &lib_ident(&lib.name), module));
            }
            Err(format!("{name} is a type the generated SDK defines"))
        }
    }
}

/// One argument of the call, as the emitter would write it: a parameter
/// under its own spelling goes through the same conversion the emitter
/// applies (`ext_render::coerce`), wherever in the argument tree it sits.
/// `Err` names a shape the probe has no declared terms for.
fn arg_expr(
    module: &Module,
    lib: &ExtLib,
    alias: &str,
    params: &[ExternParam],
    arg: &CallArg,
) -> Result<String, String> {
    match arg {
        CallArg::Param(n) => Ok(n.clone()),
        CallArg::ParamAs { name, spelling } => {
            let param = params
                .iter()
                .find(|p| &p.name == name)
                .ok_or_else(|| format!("{name} is not a parameter of the op"))?;
            coerce(module, lib, &param.r#type, spelling, name, None)
        }
        CallArg::Foreign(_) => Ok("ctx".to_string()),
        CallArg::Lit(v) => Ok(literal_of_json(v)),
        CallArg::SymbolCall(sc) => {
            let args = sc
                .args
                .iter()
                .map(|a| arg_expr(module, lib, alias, params, a))
                .collect::<Result<Vec<_>, _>>()?;
            Ok(format!(
                "{}({})",
                qualify(&sc.symbol, alias, module),
                args.join(", ")
            ))
        }
        CallArg::Ctor(c) => {
            let form = lib
                .structs
                .iter()
                .find(|s| s.name == c.name)
                .and_then(|s| s.lang("go"))
                .ok_or_else(|| format!("the struct literal {} has no go block", c.name))?;
            let fields = c
                .fields
                .iter()
                .map(|(k, v)| Ok(format!("{k}: {}", arg_expr(module, lib, alias, params, v)?)))
                .collect::<Result<Vec<_>, String>>()?;
            Ok(format!(
                "{}{{{}}}",
                qualify(&form.name, alias, module),
                fields.join(", ")
            ))
        }
        CallArg::TypeRef(_) => Err("Go has no class reference to pass".to_string()),
        CallArg::List(_) | CallArg::Ref(_) | CallArg::Call(_) => {
            Err("an argument shape the probe does not express".to_string())
        }
    }
}

/// The probe's parameter list: every declared parameter under its default
/// Go type (what the generated method receives; a spelling of its own is
/// applied at the argument, as the emitter does), plus the context when a
/// position declares it.
fn params(
    module: &Module,
    lib: &ExtLib,
    decl: &ExternDecl,
    lang: &ExternLang,
) -> Result<Vec<String>, String> {
    let mut out = Vec::new();
    for p in &decl.params {
        let ty = probe_type(module, lib, &p.r#type)
            .map_err(|why| format!("parameter {}: {why}", p.name))?;
        out.push(format!("{} {ty}", p.name));
    }
    if binds_ctx(lang) {
        out.push("ctx context.Context".to_string());
    }
    Ok(out)
}

/// The result types the call is assigned to, by the emitter's convention.
fn results(
    module: &Module,
    lib: &ExtLib,
    alias: &str,
    decl: &ExternDecl,
    lang: &ExternLang,
) -> Result<Vec<String>, String> {
    let mut out = Vec::new();
    if lang.yields.is_empty() {
        let ty = probe_type(module, lib, &decl.r#return)
            .map_err(|why| format!("the return type: {why}"))?;
        out.push(ty);
        out.push("error".to_string());
        return Ok(out);
    }
    let explicit_error = lang.yields.iter().any(|y| y.is_error);
    for y in &lang.yields {
        if y.is_error {
            out.push("error".to_string());
        } else if let Some(sp) = &y.foreign {
            out.push(qualify(sp, alias, module));
        } else if let Some(t) = &y.r#type {
            out.push(
                probe_type(module, lib, t)
                    .map_err(|why| format!("yields position {}: {why}", y.name))?,
            );
        }
    }
    if !explicit_error {
        out.push("error".to_string());
    }
    Ok(out)
}

/// The whole probe line for one binding.
fn op_line(
    module: &Module,
    lib: &ExtLib,
    alias: &str,
    owner: Option<&OpaqueType>,
    decl: &ExternDecl,
    lang: &ExternLang,
) -> Result<String, String> {
    let mut ps = params(module, lib, decl, lang)?;
    let head = match owner {
        None => qualify(&lang.symbol, alias, module),
        Some(handle) => {
            let storage = handle_go_type(lib, handle, module, &mut Vec::new())
                .ok_or_else(|| "the handle declares no go storage".to_string())?;
            ps.insert(0, format!("tonoRecv {storage}"));
            format!("tonoRecv.{}", lang.symbol)
        }
    };
    let args = lang
        .call_args
        .iter()
        .map(|a| arg_expr(module, lib, alias, &decl.params, a))
        .collect::<Result<Vec<_>, _>>()?;
    let rs = results(module, lib, alias, decl, lang)?;
    let names: Vec<String> = (0..rs.len()).map(|i| format!("tonoR{i}")).collect();
    let decls: Vec<String> = names
        .iter()
        .zip(&rs)
        .map(|(n, t)| format!("var {n} {t}"))
        .collect();
    let blanks = vec!["_"; rs.len()].join(", ");
    let fname = match owner {
        Some(h) => format!("tonoProbe_{}_{}", h.name, decl.name),
        None => format!("tonoProbe_{}", decl.name),
    };
    Ok(format!(
        "func {fname}({}) {{ {}; {} = {head}({}); {blanks} = {} }}",
        ps.join(", "),
        decls.join("; "),
        names.join(", "),
        args.join(", "),
        names.join(", ")
    ))
}

fn go_lang(decl: &ExternDecl) -> Option<&ExternLang> {
    decl.langs.iter().find(|l| l.lang == "go")
}

/// Build the Go probe for `lib`. Empty when the lib declares no Go module.
pub fn probe(module: &Module, lib: &ExtLib) -> Probe {
    let mut probe = Probe::default();
    let Some(path) = lib.langs.iter().find(|l| l.lang == "go") else {
        return probe;
    };
    let alias = lib_ident(&lib.name);
    let ops: Vec<(Option<&OpaqueType>, &ExternDecl)> = lib
        .types
        .iter()
        .flat_map(|t| t.methods.iter().map(move |m| (Some(t), m)))
        .chain(lib.externs.iter().map(|d| (None, d)))
        .collect();
    let needs_ctx = ops.iter().any(|(_, d)| go_lang(d).is_some_and(binds_ctx));

    probe.push_plain("package tonocheck");
    probe.push_plain("");
    probe.push_plain("import (");
    if needs_ctx {
        probe.push_plain("\t\"context\"");
    }
    probe.push(&SiteKey::path(), &format!("\t{alias} {:?}", path.path));
    probe.push_plain(")");
    probe.push_plain("");

    for handle in &lib.types {
        let key = SiteKey::handle(&handle.name);
        if let Some(storage) = handle_go_type(lib, handle, module, &mut Vec::new()) {
            probe.push(&key, &format!("var _ {storage}"));
        }
    }
    for form in &lib.structs {
        let Some(go) = form.lang("go") else {
            continue;
        };
        let key = SiteKey::form(&form.name);
        let ty = qualify(&go.name, &alias, module);
        let mut checks = Vec::new();
        for f in &form.fields {
            let field_ty = match go.fields.get(&f.name) {
                Some(sp) => Ok(qualify(sp, &alias, module)),
                None => probe_type(module, lib, &f.r#type),
            };
            match field_ty {
                Ok(t) => checks.push(format!("var _ {t} = tonoForm.{}", f.name)),
                Err(why) => probe.skip(&key, &format!("field {}: {why}", f.name)),
            }
        }
        probe.push(
            &key,
            &format!(
                "func tonoForm_{}(tonoForm {ty}) {{ {} }}",
                form.name,
                checks.join("; ")
            ),
        );
    }
    for (owner, decl) in ops {
        let Some(lang) = go_lang(decl) else {
            continue;
        };
        let key = SiteKey::op(owner.map(|h| h.name.as_str()), &decl.name);
        match op_line(module, lib, &alias, owner, decl, lang) {
            Ok(line) => probe.push(&key, &line),
            Err(reason) => probe.skip(&key, &reason),
        }
    }
    probe
}

/// Write the probe under `root` (a directory inside the consumer's Go
/// module) and build it.
pub fn run(root: &Path, probe: &Probe) -> std::io::Result<RunOutcome> {
    let scratch = Scratch::create(root, "go")?;
    std::fs::write(scratch.dir.join(PROBE_FILE), &probe.source)?;
    let dir_name = scratch
        .dir
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let run = ProbeRun {
        program: "go".into(),
        args: vec!["build".into(), format!("./{dir_name}")],
        cwd: root.to_path_buf(),
        file_label: format!("{dir_name}/{PROBE_FILE}"),
        parse: parse_go_errors,
    };
    Ok(run_probe(&run))
}

#[cfg(test)]
mod tests;
