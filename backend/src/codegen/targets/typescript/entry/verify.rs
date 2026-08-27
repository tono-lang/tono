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
//!
//! The probe file sits in the module's own directory of the scratch tree,
//! beside the generated `types.ts` (see `verify::generated_types`), and
//! imports from it every generated type a binding names: a reference inside
//! a spelling (`Memo<.reading>`), a parameter or return typed by the
//! module's own shape, a struct passed as a class. Those render exactly as
//! the emitted ext glue renders them, bare and imported from `./types`.

use std::cell::RefCell;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use super::ext_call::class_reference_name;
use super::ext_handle_iface::ts_lang;
use super::ts_type;
use crate::codegen::foreign_spelling;
use crate::codegen::verify::{
    parse_tsc_errors, run_probe, Probe, ProbeRun, RunOutcome, Scratch, Sdk, SiteKey,
};
use crate::ir::{
    CallArg, ExtLib, ExternDecl, ExternLang, ExternParam, Module, OpaqueType, Prim, Tref,
};

const PROBE_FILE: &str = "probe.ts";
const LIB: &str = "tonoLib";

/// What every probe line is written against: the module, the library, and
/// whether the generated types stand beside the probe. The generated names a
/// line ends up using are collected here, for the import the probe opens
/// with.
struct Ctx<'a> {
    module: &'a Module,
    lib: &'a ExtLib,
    sdk: &'a Sdk,
    generated: RefCell<BTreeSet<String>>,
}

impl<'a> Ctx<'a> {
    fn new(module: &'a Module, lib: &'a ExtLib, sdk: &'a Sdk) -> Self {
        Self {
            module,
            lib,
            sdk,
            generated: RefCell::new(BTreeSet::new()),
        }
    }

    /// The generated type `id` names (see `verify::generated_shape`),
    /// rendered as the emitter renders it and imported from the types file.
    fn generated(&self, id: &str) -> Result<String, String> {
        let name = crate::codegen::verify::generated_shape(self.module, self.sdk, id)?;
        self.generated.borrow_mut().insert(name.clone());
        Ok(name)
    }

    /// The spelling with the library's identifiers reached through the
    /// probe's namespace import and every generated-type reference rendered
    /// and imported (`sdk_terms` has already established the types are
    /// beside the probe).
    fn qualify(&self, spelling: &str) -> String {
        for name in foreign_spelling::references(spelling) {
            if let Some(rendered) = crate::codegen::entries::generated_type_name(self.module, name)
            {
                self.generated.borrow_mut().insert(rendered);
            }
        }
        foreign_spelling::qualify(
            spelling,
            &format!("{LIB}."),
            &foreign_spelling::ts_builtin,
            false,
            &crate::codegen::entries::generated_type(self.module),
        )
    }

    /// Whether a spelling can be written here: one that references a
    /// generated type (`Memo<.reading>`) needs the generated types beside
    /// the probe, and without them the binding is listed with why.
    fn sdk_terms(&self, spelling: &str) -> Result<(), String> {
        match foreign_spelling::references(spelling).first() {
            Some(name) => self
                .sdk
                .require(&format!("#({spelling}), which references .{name},")),
            None => Ok(()),
        }
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

/// The TypeScript type a declared position has in the probe. `Err` says
/// what cannot be spelled: a support type, a form with no ts block, a type
/// of another module, a generated type when the SDK is not beside the
/// probe.
fn probe_type(cx: &Ctx<'_>, t: &Tref) -> Result<String, String> {
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
        Tref::List(inner) => Ok(format!("{}[]", probe_type(cx, inner)?)),
        Tref::Param(p) => Err(format!("{p} is a type parameter")),
        // A generated map is the plain object the emitter types it as
        // (`Record`), never the `Map` class: a probe typing it as `Map`
        // would accept a library that wants one where the generated call
        // hands it a plain object.
        Tref::Map(k, v) => Ok(format!(
            "Record<{}, {}>",
            probe_type(cx, k)?,
            probe_type(cx, v)?
        )),
        // A type written inside the ext block resolves by its bare name
        // under the module (`svc#calculator`), a handle field's type outside
        // it under the lib (`mathkit#calculator`): both name this lib's own
        // handle or form. Anything else is one of the module's own shapes,
        // which the generated types file declares.
        Tref::Ref { id, args } => {
            let name = crate::codegen::entries::local_name(id);
            if let Some(handle) = cx.lib.types.iter().find(|h| h.name == name) {
                let storage = handle
                    .storage("ts")
                    .ok_or_else(|| format!("the handle {name} declares no ts block"))?;
                cx.sdk_terms(storage)?;
                return Ok(handle_alias(&handle.name));
            }
            if let Some(form) = cx.lib.structs.iter().find(|s| s.name == name) {
                let ts = ts_block(&form.langs)
                    .ok_or_else(|| format!("the struct {name} declares no ts block"))?;
                cx.sdk_terms(&ts.name)?;
                return Ok(cx.qualify(&ts.name));
            }
            let head = cx.generated(id)?;
            if args.is_empty() {
                return Ok(head);
            }
            let args = args
                .iter()
                .map(|a| probe_type(cx, a))
                .collect::<Result<Vec<_>, _>>()?;
            Ok(format!("{head}<{}>", args.join(", ")))
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
    cx: &Ctx<'_>,
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
            cx.sdk_terms(spelling)?;
            super::ext_coerce::coerce(&param.r#type, spelling, name)
        }
        CallArg::Lit(v) => Ok(literal(v)),
        // A handle's class is the head of its storage type, the library's
        // own; a generated struct's class is the one the types file declares
        // beside the interface, imported like any generated type.
        CallArg::TypeRef(name) => match cx.lib.types.iter().find(|t| &t.name == name) {
            Some(handle) => {
                if handle.storage("ts").is_none() {
                    return Err(format!("the handle {name} declares no ts block"));
                }
                Ok(format!(
                    "{LIB}.{}",
                    class_reference_name(cx.lib, cx.module, name)
                ))
            }
            None => cx.generated(&format!("{}#{name}", cx.module.name)),
        },
        CallArg::SymbolCall(sc) => {
            let args = sc
                .args
                .iter()
                .map(|a| arg_expr(cx, params, a, prelude))
                .collect::<Result<Vec<_>, _>>()?;
            cx.sdk_terms(&sc.symbol)?;
            Ok(format!("{}({})", cx.qualify(&sc.symbol), args.join(", ")))
        }
        CallArg::Ctor(c) => {
            let form = cx.lib.structs.iter().find(|s| s.name == c.name);
            let block = form
                .and_then(|s| ts_block(&s.langs))
                .ok_or_else(|| format!("the struct literal {} has no ts block", c.name))?;
            cx.sdk_terms(&block.name)?;
            let fields = c
                .fields
                .iter()
                .map(|(k, v)| {
                    let mut value = arg_expr(cx, params, v, prelude)?;
                    // A spelled field converts here exactly as the emitted
                    // literal converts it.
                    if let (Some(spelling), Some(ff)) = (
                        block.fields.get(k),
                        form.and_then(|f| f.fields.iter().find(|ff| &ff.name == k)),
                    ) {
                        cx.sdk_terms(spelling)?;
                        value = super::ext_coerce::coerce(&ff.r#type, spelling, &value)?;
                    }
                    Ok(format!("{k}: {value}"))
                })
                .collect::<Result<Vec<_>, String>>()?;
            let name = format!("tonoA{}", prelude.len());
            prelude.push(format!(
                "const {name}: {} = {{ {} }};",
                cx.qualify(&block.name),
                fields.join(", ")
            ));
            // A spelled literal crosses here exactly as the emitted call
            // passes it.
            match &c.spelling {
                None => Ok(name),
                Some(spelling) => {
                    cx.sdk_terms(spelling)?;
                    super::ext_coerce::form_coerce(&block.name, spelling, &name)
                }
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
fn params(cx: &Ctx<'_>, decl: &ExternDecl) -> Result<Vec<String>, String> {
    let mut out = Vec::new();
    for p in &decl.params {
        let ty = probe_type(cx, &p.r#type).map_err(|why| format!("parameter {}: {why}", p.name))?;
        out.push(format!("{}: {ty}", p.name));
    }
    Ok(out)
}

/// What the call's value is assigned to: the one non-error `yields`
/// position (a `ts` binding never reads an error position, see
/// `ext_call`), else the declared return; a `Promise` of it when the op is
/// asynchronous here.
fn result_type(cx: &Ctx<'_>, decl: &ExternDecl, lang: &ExternLang) -> Result<String, String> {
    let inner = match lang.yields.iter().find(|y| !y.is_error) {
        Some(y) => match (&y.foreign, &y.r#type) {
            (Some(sp), _) => {
                cx.sdk_terms(sp)?;
                cx.qualify(sp)
            }
            (None, Some(t)) => {
                probe_type(cx, t).map_err(|why| format!("yields position {}: {why}", y.name))?
            }
            (None, None) => return Err(format!("yields position {} has no type", y.name)),
        },
        None => probe_type(cx, &decl.r#return).map_err(|why| format!("the return type: {why}"))?,
    };
    Ok(if decl.is_async("ts") {
        format!("Promise<{inner}>")
    } else {
        inner
    })
}

fn op_line(
    cx: &Ctx<'_>,
    owner: Option<&OpaqueType>,
    decl: &ExternDecl,
    lang: &ExternLang,
) -> Result<String, String> {
    for sp in crate::codegen::entries::spellings::of_lang(lang) {
        cx.sdk_terms(sp)?;
    }
    // Generation refuses a chained call for TypeScript (which link is
    // awaited is undeclared); the probe has no expression to mirror, so
    // the binding is listed as skipped rather than graded against a
    // shape the emitter never writes.
    if let Some(chain) = &lang.chain {
        return Err(format!(
            "TypeScript has no chained call to probe (.{}(..) on the returned object)",
            chain.symbol
        ));
    }
    let mut ps = params(cx, decl)?;
    let mut prelude = Vec::new();
    let args = lang
        .call_args
        .iter()
        .map(|a| arg_expr(cx, &decl.params, a, &mut prelude))
        .collect::<Result<Vec<_>, _>>()?
        .join(", ");
    let call = match owner {
        Some(handle) => {
            let storage = handle
                .storage("ts")
                .ok_or_else(|| "the handle declares no ts storage".to_string())?;
            cx.sdk_terms(storage)?;
            ps.insert(0, format!("tonoRecv: {}", handle_alias(&handle.name)));
            format!("tonoRecv.{}({args})", lang.symbol)
        }
        None => match foreign_spelling::constructed(&lang.symbol) {
            Some(class) => format!("new {}({args})", cx.qualify(class)),
            None => format!("{}({args})", cx.qualify(&lang.symbol)),
        },
    };
    let ret = result_type(cx, decl, lang)?;
    let fname = match owner {
        Some(h) => format!("tonoProbe_{}_{}", h.name, decl.name),
        None => format!("tonoProbe_{}", decl.name),
    };
    let mut body = prelude;
    body.push(format!("const tonoR: {ret} = {call};"));
    // A spelled answer is what the library gives; the emitter reads it back
    // as the declared return through the same conversion
    // (`ext_coerce::coerce_back`), so the probe grades that step too, when
    // the declared return can be spelled (a return the probe cannot spell
    // leaves the library's own signature above as what there is to grade).
    if let Some(sp) = super::ext_call::spelled_answer(lang) {
        if let Ok(declared) = probe_type(cx, &decl.r#return) {
            let value = super::ext_coerce::coerce_back(&decl.r#return, &cx.qualify(sp), "tonoR")?;
            body.push(format!("const tonoV: {declared} = {value};"));
        }
    }
    Ok(format!(
        "function {fname}({}): void {{ {} }}",
        ps.join(", "),
        body.join(" ")
    ))
}

/// Build the TypeScript probe for `lib`, against the generated types when
/// `sdk` says they stand beside it. Empty when the lib declares no
/// TypeScript module.
pub fn probe(module: &Module, lib: &ExtLib, sdk: &Sdk) -> Probe {
    let mut probe = Probe::default();
    let Some(path) = lib
        .langs
        .iter()
        .find(|l| l.lang == "ts" || l.lang == "typescript")
    else {
        return probe;
    };
    let cx = Ctx::new(module, lib, sdk);
    // The body is written first: which generated types it imports is only
    // known once every line is.
    let mut body: Vec<(SiteKey, String)> = Vec::new();
    for handle in &lib.types {
        if let Some(storage) = handle.storage("ts") {
            let key = SiteKey::handle(&handle.name);
            if let Err(why) = cx.sdk_terms(storage) {
                probe.skip(&key, &why);
                continue;
            }
            body.push((
                key,
                format!(
                    "type {} = {};",
                    handle_alias(&handle.name),
                    cx.qualify(storage)
                ),
            ));
        }
    }
    for form in &lib.structs {
        let Some(ts) = ts_block(&form.langs) else {
            continue;
        };
        let key = SiteKey::form(&form.name);
        if let Err(why) = cx.sdk_terms(&ts.name) {
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
        body.push((
            key,
            format!(
                "function tonoForm_{}(tonoForm: {}): void {{ {} }}",
                form.name,
                cx.qualify(&ts.name),
                reads.join(" ")
            ),
        ));
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
        match op_line(&cx, owner, decl, lang) {
            Ok(line) => body.push((key, line)),
            Err(reason) => probe.skip(&key, &reason),
        }
    }
    probe.push(
        &SiteKey::path(),
        &format!("import * as {LIB} from {:?};", path.path),
    );
    let generated = cx.generated.into_inner();
    if !generated.is_empty() {
        let names: Vec<String> = generated.into_iter().collect();
        probe.push_plain(&format!(
            "import {{ {} }} from \"./types\";",
            names.join(", ")
        ));
    }
    probe.push_plain("");
    for (key, line) in body {
        probe.push(&key, &line);
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

fn tsconfig(probe_file: &str) -> String {
    format!(
        r#"{{
  "compilerOptions": {{
    "strict": true,
    "noEmit": true,
    "target": "ES2020",
    "module": "ES2022",
    "moduleResolution": "bundler",
    "lib": ["ES2020", "DOM"],
    "skipLibCheck": true,
    "types": []
  }},
  "files": [{probe_file:?}]
}}
"#
    )
}

/// Write the probe into the module's directory of the scratch tree (beside
/// the generated `types.ts`, when it is there) and typecheck it from the
/// scratch root, whose ancestors hold the consumer's `node_modules`.
pub fn run(scratch: &Scratch, module_dir: &Path, probe: &Probe) -> std::io::Result<RunOutcome> {
    let dir = scratch.dir.join(module_dir);
    std::fs::create_dir_all(&dir)?;
    std::fs::write(dir.join(PROBE_FILE), &probe.source)?;
    let file_label = module_dir
        .join(PROBE_FILE)
        .to_string_lossy()
        .replace('\\', "/");
    std::fs::write(scratch.dir.join("tsconfig.json"), tsconfig(&file_label))?;
    let run = ProbeRun {
        program: tsc_program(&scratch.dir),
        args: vec![
            "-p".into(),
            "tsconfig.json".into(),
            "--pretty".into(),
            "false".into(),
        ],
        cwd: scratch.dir.clone(),
        file_label,
        parse: parse_tsc_errors,
    };
    Ok(run_probe(&run))
}

#[cfg(test)]
mod tests;
