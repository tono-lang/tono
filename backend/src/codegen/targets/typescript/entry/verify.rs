//! The TypeScript probe: one line per binding, typed the way the emitter
//! crosses the boundary, checked by `tsc --noEmit` from inside the consumer
//! tree so the package resolves through the same `node_modules` the
//! generated SDK imports from.
//!
//! A probe function's parameters carry the default TypeScript mapping (a
//! handle by its declared storage); a parameter under its own spelling goes
//! through the same conversion the emitter applies, at the argument, so
//! `tsc` grades exactly what the generated call crosses; a struct literal
//! is first bound to the form it names, spelled fields converted, so the
//! object is checked against the library's own type; the
//! call's value is assigned to the declared return, wrapped in `Promise`
//! when the op declares the call asynchronous here (see `ext_call`). A
//! binding the probe cannot express in declared terms is listed as skipped.

use std::path::{Path, PathBuf};

use super::ext_call::class_reference_name;
use super::ext_handle_iface::ts_lang;
use super::ts_type;
use crate::codegen::foreign_spelling;
use crate::codegen::verify::{
    parse_tsc_errors, run_probe, Probe, ProbeRun, RunOutcome, Scratch, SiteKey,
};
use crate::ir::{
    CallArg, ExtLib, ExternDecl, ExternLang, ExternParam, Module, OpaqueType, Prim, Tref,
};

const PROBE_FILE: &str = "probe.ts";
const LIB: &str = "tonoLib";

/// The spelling with the library's identifiers reached through the probe's
/// namespace import. A reference to a generated type never gets here:
/// `sdk_terms` lists the binding as skipped first.
fn qualify(spelling: &str, module: &Module) -> String {
    foreign_spelling::qualify(
        spelling,
        &format!("{LIB}."),
        &foreign_spelling::ts_builtin,
        false,
        &crate::codegen::entries::generated_type(module),
    )
}

/// Whether a spelling can be written without the generated SDK: one that
/// references a generated type (`Memo<.reading>`) cannot, and the binding
/// is listed with why.
fn sdk_terms(spelling: &str) -> Result<(), String> {
    match foreign_spelling::references(spelling).first() {
        Some(name) => Err(format!(
            "#({spelling}) references .{name}, a type the generated SDK defines"
        )),
        None => Ok(()),
    }
}

fn handle_alias(name: &str) -> String {
    format!("TonoHandle_{name}")
}

fn ts_block(langs: &[crate::ir::ForeignLang]) -> Option<&crate::ir::ForeignLang> {
    langs
        .iter()
        .find(|l| l.lang == "ts" || l.lang == "typescript")
}

/// The TypeScript type a declared position has in the probe, when it can
/// be spelled without the generated SDK. `Err` says what cannot be: a tono
/// shape (a generated type), a support type, a form with no ts block.
fn probe_type(module: &Module, lib: &ExtLib, t: &Tref) -> Result<String, String> {
    match t {
        Tref::Prim(p) => match p {
            Prim::Bool
            | Prim::String
            | Prim::I8
            | Prim::I16
            | Prim::I32
            | Prim::I64
            | Prim::U8
            | Prim::U16
            | Prim::U32
            | Prim::U64
            | Prim::Float => Ok(ts_type(t)),
            _ => Err(format!(
                "{} is spelled by the generated SDK's support code",
                ts_type(t)
            )),
        },
        Tref::List(inner) => Ok(format!("{}[]", probe_type(module, lib, inner)?)),
        Tref::Param(p) => Err(format!("{p} is a type parameter")),
        Tref::Map(k, v) => Ok(format!(
            "Map<{}, {}>",
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
                let storage = handle
                    .storage("ts")
                    .ok_or_else(|| format!("the handle {name} declares no ts block"))?;
                sdk_terms(storage)?;
                return Ok(handle_alias(&handle.name));
            }
            if let Some(form) = lib.structs.iter().find(|s| s.name == name) {
                let ts = ts_block(&form.langs)
                    .ok_or_else(|| format!("the struct {name} declares no ts block"))?;
                sdk_terms(&ts.name)?;
                return Ok(qualify(&ts.name, module));
            }
            Err(format!("{name} is a type the generated SDK defines"))
        }
    }
}

fn literal(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::String(s) => format!("{s:?}"),
        other => other.to_string(),
    }
}

/// One argument as an expression, plus the typed bindings a struct literal
/// needs ahead of the call (`const tonoA0: Lib.Form = {...}`).
fn arg_expr(
    module: &Module,
    lib: &ExtLib,
    params: &[ExternParam],
    arg: &CallArg,
    prelude: &mut Vec<String>,
) -> Result<String, String> {
    match arg {
        CallArg::Param(n) => Ok(n.clone()),
        // A parameter under its own spelling goes through the same
        // conversion the emitter applies (`ext_coerce::coerce`): the
        // probe's parameter keeps the default type, so `tsc` grades
        // exactly what the generated call crosses.
        CallArg::ParamAs { name, spelling } => {
            let param = params
                .iter()
                .find(|p| &p.name == name)
                .ok_or_else(|| format!("{name} is not a parameter of the op"))?;
            super::ext_coerce::coerce(&param.r#type, spelling, name)
        }
        CallArg::Lit(v) => Ok(literal(v)),
        CallArg::TypeRef(handle) => {
            let declared = lib
                .types
                .iter()
                .find(|t| &t.name == handle)
                .and_then(|t| t.storage("ts"));
            if declared.is_none() {
                return Err(format!("the handle {handle} declares no ts block"));
            }
            Ok(format!("{LIB}.{}", class_reference_name(lib, handle)))
        }
        CallArg::SymbolCall(sc) => {
            let args = sc
                .args
                .iter()
                .map(|a| arg_expr(module, lib, params, a, prelude))
                .collect::<Result<Vec<_>, _>>()?;
            Ok(format!(
                "{}({})",
                qualify(&sc.symbol, module),
                args.join(", ")
            ))
        }
        CallArg::Ctor(c) => {
            let form = lib.structs.iter().find(|s| s.name == c.name);
            let block = form
                .and_then(|s| ts_block(&s.langs))
                .ok_or_else(|| format!("the struct literal {} has no ts block", c.name))?;
            sdk_terms(&block.name)?;
            let fields = c
                .fields
                .iter()
                .map(|(k, v)| {
                    let mut value = arg_expr(module, lib, params, v, prelude)?;
                    // A spelled field converts here exactly as the emitted
                    // literal converts it.
                    if let (Some(spelling), Some(ff)) = (
                        block.fields.get(k),
                        form.and_then(|f| f.fields.iter().find(|ff| &ff.name == k)),
                    ) {
                        value = super::ext_coerce::coerce(&ff.r#type, spelling, &value)?;
                    }
                    Ok(format!("{k}: {value}"))
                })
                .collect::<Result<Vec<_>, String>>()?;
            let name = format!("tonoA{}", prelude.len());
            prelude.push(format!(
                "const {name}: {} = {{ {} }};",
                qualify(&block.name, module),
                fields.join(", ")
            ));
            // A spelled literal crosses here exactly as the emitted call
            // passes it.
            match &c.spelling {
                None => Ok(name),
                Some(spelling) => super::ext_coerce::form_coerce(&block.name, spelling, &name),
            }
        }
        CallArg::Foreign(_) => Err("TypeScript binds no position of its own".to_string()),
        CallArg::List(_) | CallArg::Ref(_) | CallArg::Call(_) => {
            Err("an argument shape the probe does not express".to_string())
        }
    }
}

/// The probe's parameter list: every declared parameter under its default
/// TypeScript type (what the generated call site holds; a spelling of its
/// own is applied at the argument, as the emitter does).
fn params(module: &Module, lib: &ExtLib, decl: &ExternDecl) -> Result<Vec<String>, String> {
    let mut out = Vec::new();
    for p in &decl.params {
        let ty = probe_type(module, lib, &p.r#type)
            .map_err(|why| format!("parameter {}: {why}", p.name))?;
        out.push(format!("{}: {ty}", p.name));
    }
    Ok(out)
}

/// What the call's value is assigned to: the one non-error `yields`
/// position (a `ts` binding never reads an error position, see
/// `ext_call`), else the declared return; a `Promise` of it when the op is
/// asynchronous here.
fn result_type(
    module: &Module,
    lib: &ExtLib,
    decl: &ExternDecl,
    lang: &ExternLang,
) -> Result<String, String> {
    let inner = match lang.yields.iter().find(|y| !y.is_error) {
        Some(y) => match (&y.foreign, &y.r#type) {
            (Some(sp), _) => qualify(sp, module),
            (None, Some(t)) => probe_type(module, lib, t)
                .map_err(|why| format!("yields position {}: {why}", y.name))?,
            (None, None) => return Err(format!("yields position {} has no type", y.name)),
        },
        None => probe_type(module, lib, &decl.r#return)
            .map_err(|why| format!("the return type: {why}"))?,
    };
    Ok(if decl.is_async("ts") {
        format!("Promise<{inner}>")
    } else {
        inner
    })
}

fn op_line(
    module: &Module,
    lib: &ExtLib,
    owner: Option<&OpaqueType>,
    decl: &ExternDecl,
    lang: &ExternLang,
) -> Result<String, String> {
    for sp in crate::codegen::entries::spellings::of_lang(lang) {
        sdk_terms(sp)?;
    }
    let mut ps = params(module, lib, decl)?;
    let mut prelude = Vec::new();
    let args = lang
        .call_args
        .iter()
        .map(|a| arg_expr(module, lib, &decl.params, a, &mut prelude))
        .collect::<Result<Vec<_>, _>>()?
        .join(", ");
    let call = match owner {
        Some(handle) => {
            let storage = handle
                .storage("ts")
                .ok_or_else(|| "the handle declares no ts storage".to_string())?;
            sdk_terms(storage)?;
            ps.insert(0, format!("tonoRecv: {}", handle_alias(&handle.name)));
            format!("tonoRecv.{}({args})", lang.symbol)
        }
        None => match foreign_spelling::constructed(&lang.symbol) {
            Some(class) => format!("new {}({args})", qualify(class, module)),
            None => format!("{}({args})", qualify(&lang.symbol, module)),
        },
    };
    let ret = result_type(module, lib, decl, lang)?;
    let fname = match owner {
        Some(h) => format!("tonoProbe_{}_{}", h.name, decl.name),
        None => format!("tonoProbe_{}", decl.name),
    };
    let mut body = prelude;
    body.push(format!("const tonoR: {ret} = {call};"));
    Ok(format!(
        "function {fname}({}): void {{ {} }}",
        ps.join(", "),
        body.join(" ")
    ))
}

/// Build the TypeScript probe for `lib`. Empty when the lib declares no
/// TypeScript module.
pub fn probe(module: &Module, lib: &ExtLib) -> Probe {
    let mut probe = Probe::default();
    let Some(path) = lib
        .langs
        .iter()
        .find(|l| l.lang == "ts" || l.lang == "typescript")
    else {
        return probe;
    };
    probe.push(
        &SiteKey::path(),
        &format!("import * as {LIB} from {:?};", path.path),
    );
    probe.push_plain("");
    for handle in &lib.types {
        if let Some(storage) = handle.storage("ts") {
            let key = SiteKey::handle(&handle.name);
            if let Err(why) = sdk_terms(storage) {
                probe.skip(&key, &why);
                continue;
            }
            probe.push(
                &key,
                &format!(
                    "type {} = {};",
                    handle_alias(&handle.name),
                    qualify(storage, module)
                ),
            );
        }
    }
    for form in &lib.structs {
        let Some(ts) = ts_block(&form.langs) else {
            continue;
        };
        let key = SiteKey::form(&form.name);
        if let Err(why) = sdk_terms(&ts.name) {
            probe.skip(&key, &why);
            continue;
        }
        // A form is read (a `yields` position) or built (a struct literal,
        // checked at its call); here the fields are checked to exist.
        let reads: Vec<String> = form
            .fields
            .iter()
            .map(|f| format!("void tonoForm.{};", f.name))
            .collect();
        probe.push(
            &key,
            &format!(
                "function tonoForm_{}(tonoForm: {}): void {{ {} }}",
                form.name,
                qualify(&ts.name, module),
                reads.join(" ")
            ),
        );
    }
    let ops: Vec<(Option<&OpaqueType>, &ExternDecl)> = lib
        .types
        .iter()
        .flat_map(|t| t.methods.iter().map(move |m| (Some(t), m)))
        .chain(lib.externs.iter().map(|d| (None, d)))
        .collect();
    for (owner, decl) in ops {
        let Some(lang) = ts_lang(decl) else {
            continue;
        };
        let key = SiteKey::op(owner.map(|h| h.name.as_str()), &decl.name);
        match op_line(module, lib, owner, decl, lang) {
            Ok(line) => probe.push(&key, &line),
            Err(reason) => probe.skip(&key, &reason),
        }
    }
    probe
}

/// The `tsc` the consumer tree installed, else the one on `PATH`.
fn tsc_program(root: &Path) -> PathBuf {
    root.ancestors()
        .map(|d| d.join("node_modules").join(".bin").join("tsc"))
        .find(|p| p.is_file())
        .unwrap_or_else(|| PathBuf::from("tsc"))
}

const TSCONFIG: &str = r#"{
  "compilerOptions": {
    "strict": true,
    "noEmit": true,
    "target": "ES2020",
    "module": "ES2022",
    "moduleResolution": "bundler",
    "lib": ["ES2020", "DOM"],
    "skipLibCheck": true,
    "types": []
  },
  "files": ["probe.ts"]
}
"#;

/// Write the probe under `root` (a directory whose ancestors hold the
/// consumer's `node_modules`) and typecheck it.
pub fn run(root: &Path, probe: &Probe) -> std::io::Result<RunOutcome> {
    let scratch = Scratch::create(root, "ts")?;
    std::fs::write(scratch.dir.join(PROBE_FILE), &probe.source)?;
    std::fs::write(scratch.dir.join("tsconfig.json"), TSCONFIG)?;
    let run = ProbeRun {
        program: tsc_program(root),
        args: vec![
            "-p".into(),
            "tsconfig.json".into(),
            "--pretty".into(),
            "false".into(),
        ],
        cwd: scratch.dir.clone(),
        file_label: PROBE_FILE.to_string(),
        parse: parse_tsc_errors,
    };
    Ok(run_probe(&run))
}

#[cfg(test)]
mod tests;
