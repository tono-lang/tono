//! The Go probe: one line per binding, typed the way the emitter crosses
//! the boundary, compiled by `go build` inside the consumer's module so the
//! library resolves exactly as the generated SDK's import does.
//!
//! A probe function's parameters carry what the binding says crosses (a
//! parameter's own spelling, else the default Go mapping, a handle's
//! declared storage, the context where a position declares it); the call
//! is assigned to results typed by the target's convention (see
//! `ext::build_call`: an implicit trailing `error` unless a `yields:`
//! position marks it, or the `yields:` list is the call's whole signature).
//! Nothing is inferred from the library: a binding the probe cannot express
//! in declared terms is listed as skipped, with why.
//!
//! The probe file is a file of the module's own package in the scratch
//! tree, beside the generated `types.go` (see `verify::generated_types`), so
//! a generated type a binding names (a reference inside a spelling,
//! `Memo[.reading]`; a parameter typed by the module's own shape) is written
//! bare, exactly as the emitted ext glue writes it in that package.

use std::path::Path;

use super::ext::ext_render::{coerce, form_coerce, literal_of_json};
use super::ext::{binds_ctx, handle_go_type, lib_ident, qualify};
use super::{go_type, go_type_label};
use crate::codegen::verify::{
    parse_go_errors, run_probe, Probe, ProbeRun, RunOutcome, Scratch, Sdk, SiteKey,
};
use crate::ir::{
    CallArg, ExtLib, ExternDecl, ExternLang, ExternParam, Module, OpaqueType, Prim, Tref,
};

const PROBE_FILE: &str = "probe.go";

/// What every probe line is written against: the module, the library, its
/// import alias, and whether the generated types stand beside the probe.
struct Ctx<'a> {
    module: &'a Module,
    lib: &'a ExtLib,
    alias: String,
    sdk: &'a Sdk,
}

impl Ctx<'_> {
    /// Whether a spelling can be written here: one that references a
    /// generated type (`Memo[.reading]`) needs the generated types beside
    /// the probe, and without them the binding is listed with why.
    fn sdk_terms(&self, spelling: &str) -> Result<(), String> {
        match crate::codegen::foreign_spelling::references(spelling).first() {
            Some(name) => self
                .sdk
                .require(&format!("#({spelling}), which references .{name},")),
            None => Ok(()),
        }
    }

    fn qualify(&self, spelling: &str) -> String {
        qualify(spelling, &self.alias, self.module)
    }

    /// The handle's declared Go storage, qualified, when the probe can spell
    /// it: `Err` names a reference the types cannot answer or a missing go
    /// block.
    fn handle_storage(&self, handle: &OpaqueType) -> Result<String, String> {
        let storage = handle
            .storage("go")
            .ok_or_else(|| format!("the handle {} declares no go block", handle.name))?;
        self.sdk_terms(storage)?;
        handle_go_type(self.lib, handle, self.module, &mut Vec::new())
            .ok_or_else(|| "the lib declares no go module path".to_string())
    }
}

/// The Go type a declared position has in the probe: builtins, lists and
/// maps of them, handles by their storage, foreign forms by their name, the
/// module's own shapes by the name the types file declares. `Err` says what
/// cannot be spelled: a support type, a form with no go block, a type of
/// another module, a generated type when the SDK is not beside the probe.
fn probe_type(cx: &Ctx<'_>, t: &Tref) -> Result<String, String> {
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
        Tref::List(inner) => Ok(format!("[]{}", probe_type(cx, inner)?)),
        Tref::Param(p) => Err(format!("{p} is a type parameter")),
        Tref::Map(k, v) => Ok(format!("map[{}]{}", probe_type(cx, k)?, probe_type(cx, v)?)),
        // A type written inside the ext block resolves by its bare name
        // under the module (`svc#calculator`), a handle field's type outside
        // it under the lib (`mathkit#calculator`): both name this lib's own
        // handle or form. Anything else is one of the module's own shapes,
        // which the generated types file declares in this package.
        Tref::Ref { id, args } => {
            let name = crate::codegen::entries::local_name(id);
            if let Some(handle) = cx.lib.types.iter().find(|h| h.name == name) {
                return cx.handle_storage(handle);
            }
            if let Some(form) = cx.lib.structs.iter().find(|s| s.name == name) {
                let go = form
                    .lang("go")
                    .ok_or_else(|| format!("the struct {name} declares no go block"))?;
                cx.sdk_terms(&go.name)?;
                return Ok(cx.qualify(&go.name));
            }
            let head = crate::codegen::verify::generated_shape(cx.module, cx.sdk, id)?;
            if args.is_empty() {
                return Ok(head);
            }
            let args = args
                .iter()
                .map(|a| probe_type(cx, a))
                .collect::<Result<Vec<_>, _>>()?;
            Ok(format!("{head}[{}]", args.join(", ")))
        }
    }
}

/// One argument of the call, as the emitter would write it: a parameter
/// under its own spelling goes through the same conversion the emitter
/// applies (`ext_render::coerce`), wherever in the argument tree it sits.
/// `Err` names a shape the probe has no declared terms for.
fn arg_expr(cx: &Ctx<'_>, params: &[ExternParam], arg: &CallArg) -> Result<String, String> {
    match arg {
        CallArg::Param(n) => Ok(n.clone()),
        CallArg::ParamAs { name, spelling } => {
            let param = params
                .iter()
                .find(|p| &p.name == name)
                .ok_or_else(|| format!("{name} is not a parameter of the op"))?;
            cx.sdk_terms(spelling)?;
            coerce(cx.module, cx.lib, &param.r#type, spelling, name, None)
        }
        CallArg::Foreign(_) => Ok("ctx".to_string()),
        CallArg::Lit(v) => Ok(literal_of_json(v)),
        CallArg::SymbolCall(sc) => {
            let args = sc
                .args
                .iter()
                .map(|a| arg_expr(cx, params, a))
                .collect::<Result<Vec<_>, _>>()?;
            cx.sdk_terms(&sc.symbol)?;
            Ok(format!("{}({})", cx.qualify(&sc.symbol), args.join(", ")))
        }
        CallArg::Ctor(c) => {
            // A literal of one of the module's own structs is a value the
            // generated SDK defines, like a parameter of that type.
            let form = cx
                .lib
                .structs
                .iter()
                .find(|s| s.name == c.name)
                .ok_or_else(|| {
                    format!(
                        "the struct literal {} builds a type the generated SDK defines",
                        c.name
                    )
                })?
                .lang("go")
                .ok_or_else(|| format!("the struct literal {} has no go block", c.name))?;
            cx.sdk_terms(&form.name)?;
            let fields = c
                .fields
                .iter()
                .map(|(k, v)| Ok(format!("{k}: {}", arg_expr(cx, params, v)?)))
                .collect::<Result<Vec<_>, String>>()?;
            let literal = format!("{}{{{}}}", cx.qualify(&form.name), fields.join(", "));
            // A spelled literal crosses here exactly as the emitted call
            // passes it (`&mathkit.Options{..}`); the form itself is still
            // probed as the value type its block declares.
            match &c.spelling {
                None => Ok(literal),
                Some(spelling) => {
                    cx.sdk_terms(spelling)?;
                    form_coerce(cx.module, cx.lib, form, spelling, &literal)
                }
            }
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
fn params(cx: &Ctx<'_>, decl: &ExternDecl, lang: &ExternLang) -> Result<Vec<String>, String> {
    let mut out = Vec::new();
    for p in &decl.params {
        let ty = probe_type(cx, &p.r#type).map_err(|why| format!("parameter {}: {why}", p.name))?;
        out.push(format!("{} {ty}", p.name));
    }
    if binds_ctx(lang) {
        out.push("ctx context.Context".to_string());
    }
    Ok(out)
}

/// The result types the call is assigned to, by the emitter's convention.
fn results(cx: &Ctx<'_>, decl: &ExternDecl, lang: &ExternLang) -> Result<Vec<String>, String> {
    let mut out = Vec::new();
    if lang.yields.is_empty() {
        let ty = probe_type(cx, &decl.r#return).map_err(|why| format!("the return type: {why}"))?;
        out.push(ty);
        out.push("error".to_string());
        return Ok(out);
    }
    let explicit_error = lang.yields.iter().any(|y| y.is_error);
    for y in &lang.yields {
        if y.is_error {
            out.push("error".to_string());
        } else if let Some(sp) = &y.foreign {
            cx.sdk_terms(sp)?;
            out.push(cx.qualify(sp));
        } else if let Some(t) = &y.r#type {
            out.push(
                probe_type(cx, t).map_err(|why| format!("yields position {}: {why}", y.name))?,
            );
        }
    }
    // A projecting binding keeps the convention's trailing error; a list
    // that is the signature has only what it declares (see `ext::build_call`).
    if !explicit_error && !lang.yields_is_signature() {
        out.push("error".to_string());
    }
    Ok(out)
}

/// The whole probe line for one binding.
fn op_line(
    cx: &Ctx<'_>,
    owner: Option<&OpaqueType>,
    decl: &ExternDecl,
    lang: &ExternLang,
) -> Result<String, String> {
    for sp in crate::codegen::entries::spellings::of_lang(lang) {
        cx.sdk_terms(sp)?;
    }
    let mut ps = params(cx, decl, lang)?;
    let head = match owner {
        None => cx.qualify(&lang.symbol),
        Some(handle) => {
            let storage = cx.handle_storage(handle)?;
            ps.insert(0, format!("tonoRecv {storage}"));
            format!("tonoRecv.{}", lang.symbol)
        }
    };
    let args = lang
        .call_args
        .iter()
        .map(|a| arg_expr(cx, &decl.params, a))
        .collect::<Result<Vec<_>, _>>()?;
    // The chained method is probed exactly as the emitter writes it: one
    // expression, the results bound off its last link.
    let chain = match &lang.chain {
        None => String::new(),
        Some(chain) => {
            let chain_args = chain
                .args
                .iter()
                .map(|a| arg_expr(cx, &decl.params, a))
                .collect::<Result<Vec<_>, _>>()?;
            format!(".{}({})", chain.symbol, chain_args.join(", "))
        }
    };
    let rs = results(cx, decl, lang)?;
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
        "func {fname}({}) {{ {}; {} = {head}({}){chain}; {blanks} = {} }}",
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

/// Build the Go probe for `lib` as a file of `package`, the module's own
/// package where the generated types are declared when `sdk` says they
/// stand beside it. Empty when the lib declares no Go module.
pub fn probe(module: &Module, lib: &ExtLib, sdk: &Sdk, package: &str) -> Probe {
    let mut probe = Probe::default();
    let Some(path) = lib.langs.iter().find(|l| l.lang == "go") else {
        return probe;
    };
    let cx = Ctx {
        module,
        lib,
        alias: lib_ident(&lib.name),
        sdk,
    };
    let ops: Vec<(Option<&OpaqueType>, &ExternDecl)> = lib
        .types
        .iter()
        .flat_map(|t| t.methods.iter().map(move |m| (Some(t), m)))
        .chain(lib.externs.iter().map(|d| (None, d)))
        .collect();
    // The body is written first: which imports it uses is only known once
    // every line is, and Go refuses an import nothing uses.
    let mut body: Vec<(SiteKey, String)> = Vec::new();
    for handle in &lib.types {
        let key = SiteKey::handle(&handle.name);
        if handle.storage("go").is_none() {
            continue;
        }
        match cx.handle_storage(handle) {
            Ok(storage) => body.push((key, format!("var _ {storage}"))),
            Err(why) => probe.skip(&key, &why),
        }
    }
    for form in &lib.structs {
        let Some(go) = form.lang("go") else {
            continue;
        };
        let key = SiteKey::form(&form.name);
        if let Err(why) = cx.sdk_terms(&go.name) {
            probe.skip(&key, &why);
            continue;
        }
        let ty = cx.qualify(&go.name);
        let mut checks = Vec::new();
        for f in &form.fields {
            let field_ty = match go.fields.get(&f.name) {
                Some(sp) => cx.sdk_terms(sp).map(|()| cx.qualify(sp)),
                None => probe_type(&cx, &f.r#type),
            };
            match field_ty {
                Ok(t) => checks.push(format!("var _ {t} = tonoForm.{}", f.name)),
                Err(why) => probe.skip(&key, &format!("field {}: {why}", f.name)),
            }
        }
        body.push((
            key,
            format!(
                "func tonoForm_{}(tonoForm {ty}) {{ {} }}",
                form.name,
                checks.join("; ")
            ),
        ));
    }
    for (owner, decl) in ops {
        let Some(lang) = go_lang(decl) else {
            continue;
        };
        let key = SiteKey::op(owner.map(|h| h.name.as_str()), &decl.name);
        match op_line(&cx, owner, decl, lang) {
            Ok(line) => body.push((key, line)),
            Err(reason) => probe.skip(&key, &reason),
        }
    }
    let needs_ctx = body.iter().any(|(_, l)| l.contains("ctx context.Context"));
    // A library no line reaches (every binding skipped) is still imported,
    // blank, so the import line stands for the module path either way: an
    // unresolvable path is reported there, and a resolvable one is not an
    // unused-import error masquerading as a missing library.
    let selector = format!("{}.", cx.alias);
    let alias = if body.iter().any(|(_, l)| l.contains(&selector)) {
        cx.alias.as_str()
    } else {
        "_"
    };

    probe.push_plain(&format!("package {package}"));
    probe.push_plain("");
    probe.push_plain("import (");
    if needs_ctx {
        probe.push_plain("\t\"context\"");
    }
    probe.push(&SiteKey::path(), &format!("\t{alias} {:?}", path.path));
    probe.push_plain(")");
    probe.push_plain("");
    for (key, line) in body {
        probe.push(&key, &line);
    }
    probe
}

/// Write the probe into the module's package directory of the scratch tree
/// (beside the generated `types.go`, when it is there) and build that
/// package from the consumer's root, so the library resolves through the
/// consumer's module.
pub fn run(scratch: &Scratch, module_dir: &Path, probe: &Probe) -> std::io::Result<RunOutcome> {
    let dir = scratch.dir.join(module_dir);
    std::fs::create_dir_all(&dir)?;
    std::fs::write(dir.join(PROBE_FILE), &probe.source)?;
    let root = scratch
        .dir
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| scratch.dir.clone());
    let package_dir = Path::new(
        scratch
            .dir
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default()
            .as_str(),
    )
    .join(module_dir)
    .to_string_lossy()
    .replace('\\', "/");
    let run = ProbeRun {
        program: "go".into(),
        args: vec!["build".into(), format!("./{package_dir}")],
        cwd: root,
        file_label: format!("{package_dir}/{PROBE_FILE}"),
        parse: parse_go_errors,
    };
    Ok(run_probe(&run))
}

#[cfg(test)]
mod tests;
