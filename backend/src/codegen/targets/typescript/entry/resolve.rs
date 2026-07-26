//! The TypeScript leaf spellings for the shared resolution plan.
//!
//! The control flow (source ordering, absence tracking, config composition)
//! lives once in `codegen::entries::plan`; this file only says how each leaf
//! statement reads in TypeScript and keeps the genuinely TS-specific constructs
//! (the set-flag guaranteed chain, the `readEnv` boundary, the regex/BigInt
//! parses, the manual strict-decode checks) that no other target shares. The
//! generated file is prettier-formatted downstream, so the leaf indentation
//! only needs to be structurally valid.

use super::checks::*;
use super::*;
use crate::codegen::entries::plan::{self, Cond, Emitter, Leaf};

/// The TypeScript resolution emitter: holds the entry model and flags the
/// shared helpers the leaves use. `body` receives the rendered plan per field.
pub(super) struct Resolver<'a, 'b> {
    pub(super) entry: &'a EntryModel<'a>,
    pub(super) module: &'a Module,
    pub(super) config: &'a CasingConfig,
    pub(super) helpers: &'b mut Helpers,
    pub(super) body: &'b mut String,
}

impl Resolver<'_, '_> {
    fn ident(&self, name: &str) -> String {
        format!("s.{}", field_camel(name, self.config))
    }

    fn path_expr(&self, path: &[String]) -> String {
        let mut out = "s".to_string();
        for seg in path {
            out.push('.');
            out.push_str(&field_camel(seg, self.config));
        }
        out
    }

    fn path_type(&self, path: &[String]) -> Tref {
        self.entry.path_type(path, self.module)
    }

    fn guaranteed(&self, name: &str) -> bool {
        self.entry.field_guaranteed(name)
    }

    fn with_access(&self, field: &EntryField) -> String {
        format!("config.{}", field_camel(&field.name, self.config))
    }

    /// Build the plan for every field and render it into `body`.
    pub(super) fn emit_field(&mut self, field: &EntryField) {
        let entry = self.entry;
        let module = self.module;
        let stmt = plan::build_field(field, entry, module, self);
        let text = plan::render(&stmt, 1, self);
        self.body.push_str(&text);
    }

    /// The prereq guard when the env variable's own name comes from a sibling
    /// field that may itself be absent; the env step chains onto its `else`.
    fn env_name_prereq(&self, name: &EnvName, why: &str) -> String {
        let EnvName::Field(fr) = name else {
            return String::new();
        };
        let Some(head) = fr.field.first() else {
            return String::new();
        };
        if self.guaranteed(head) {
            return String::new();
        }
        format!(
            "if ({head_why} !== \"\") {{\n  {why} = \"{head} <- \" + {head_why};\n}} else ",
            head_why = why_var(head),
        )
    }

    /// Parse a raw env string `v` into the destination, by the declared type; a
    /// parse failure fails construction naming the variable and the type. The
    /// body sits inside `if (v !== undefined) { .. }`, relative to column zero.
    fn env_parse(&mut self, field: &EntryField, dest: &str, label: &str) -> String {
        let t = &field.target;
        match t {
            Tref::Prim(Prim::Bool) => format!(
                "if (v === \"true\" || v === \"1\") {{\n  {dest} = true;\n}} else if (v === \"false\" || v === \"0\") {{\n  {dest} = false;\n}} else {{\n  throw new Error(`${{{label}}}: invalid bool ${{JSON.stringify(v)}} (want true/false/1/0)`);\n}}"
            ),
            Tref::Prim(p @ (Prim::I8 | Prim::I16 | Prim::I32 | Prim::U8 | Prim::U16 | Prim::U32)) => {
                // Decimal digits only plus the type's own range, matching the Go
                // boundary (strconv with an explicit bit size, which takes a
                // sign only for the signed types).
                let (min, max) = int_bounds(p);
                let regex = if matches!(p, Prim::I8 | Prim::I16 | Prim::I32) {
                    "/^[+-]?[0-9]+$/"
                } else {
                    "/^[0-9]+$/"
                };
                format!(
                    "{{\n  if (!{regex}.test(v)) {{\n    throw new Error(`${{{label}}}: invalid {prim} ${{JSON.stringify(v)}}`);\n  }}\n  const n = Number(v);\n  if (!Number.isInteger(n) || n < {min} || n > {max}) {{\n    throw new Error(`${{{label}}}: invalid {prim} ${{JSON.stringify(v)}}`);\n  }}\n  {dest} = n;\n}}",
                    prim = prim_name(p),
                )
            }
            Tref::Prim(p @ (Prim::I64 | Prim::U64)) => {
                let (regex, min, max) = if matches!(p, Prim::I64) {
                    ("/^[+-]?[0-9]+$/", "-9223372036854775808n", "9223372036854775807n")
                } else {
                    // ParseUint takes no sign at all, so neither does this.
                    ("/^[0-9]+$/", "0n", "18446744073709551615n")
                };
                format!(
                    "{{\n  if (!{regex}.test(v)) {{\n    throw new Error(`${{{label}}}: invalid {prim} ${{JSON.stringify(v)}}`);\n  }}\n  const n = BigInt(v.startsWith(\"+\") ? v.slice(1) : v);\n  if (n < {min} || n > {max}) {{\n    throw new Error(`${{{label}}}: invalid {prim} ${{JSON.stringify(v)}}`);\n  }}\n  {dest} = n;\n}}",
                    prim = prim_name(p),
                )
            }
            Tref::Prim(Prim::Float) => format!(
                // Decimal notation only: bare Number() also accepts hex and
                // Infinity spellings the Go boundary rejects.
                "{{\n  if (!/^[+-]?(\\d+(\\.\\d*)?|\\.\\d+)([eE][+-]?\\d+)?$/.test(v)) {{\n    throw new Error(`${{{label}}}: invalid float ${{JSON.stringify(v)}}`);\n  }}\n  const n = Number(v);\n  if (!Number.isFinite(n)) {{\n    throw new Error(`${{{label}}}: invalid float ${{JSON.stringify(v)}}`);\n  }}\n  {dest} = n;\n}}"
            ),
            Tref::Prim(Prim::Bytes) => format!(
                // The env boundary carries bytes the same way the wire does:
                // base64 text.
                "try {{\n  {dest} = decodeBytes(v);\n}} catch {{\n  throw new Error(`${{{label}}}: invalid base64 ${{JSON.stringify(v)}}`);\n}}"
            ),
            Tref::Prim(Prim::Duration) => {
                self.helpers.duration_ms = true;
                format!(
                    "try {{\n  durationToMs(v);\n}} catch {{\n  throw new Error(`${{{label}}}: invalid duration ${{JSON.stringify(v)}}`);\n}}\n{dest} = v as Duration;"
                )
            }
            _ => format!("{dest} = {};", cast_string(t, "v")),
        }
    }

    /// A non-guaranteed chain body (used inside a match arm), relative to column
    /// zero.
    fn chain_sequential(&mut self, field: &EntryField, dest: &str, why: &str) -> String {
        let mut out = String::new();
        let mut first = true;
        for source in &field.sources {
            let step = match source {
                Source::With => self.with_step_body(field, dest, why),
                Source::Env(name) => self.env_step_body(field, name, dest, why),
                Source::Default(v) => {
                    format!("{dest} = {};\n{why} = \"\";", literal(&field.target, v))
                }
                Source::Arg => continue,
            };
            if first {
                out.push_str(&step);
                out.push('\n');
                first = false;
            } else {
                out.push_str(&format!("if ({why} !== \"\") {{\n{}\n}}\n", nest(&step, 1)));
            }
        }
        out
    }

    fn arm_stmts(
        &mut self,
        field: &EntryField,
        value: &ArmValue,
        dest: &str,
        why: &str,
        guaranteed: bool,
    ) -> String {
        match value {
            ArmValue::Lit(v) => format!("{dest} = {};", literal(&field.target, v)),
            ArmValue::Field(path) => {
                let head = path.first().cloned().unwrap_or_default();
                let expr = self.path_expr(path);
                if self.guaranteed(&head) {
                    format!("{dest} = {expr};")
                } else {
                    format!(
                        "if ({head_why} !== \"\") {{\n  {why} = \"{head} <- \" + {head_why};\n}} else {{\n  {dest} = {expr};\n}}",
                        head_why = why_var(&head),
                    )
                }
            }
            ArmValue::Sources(sources) => {
                let stub = plan::arm_sources(field, sources);
                if guaranteed {
                    self.chain_guaranteed(&stub, dest)
                } else {
                    format!(
                        "{why} = \"no source\";\n{}",
                        self.chain_sequential(&stub, dest, why)
                    )
                }
            }
        }
    }

    /// The template split: each part as a TS string expression, plus the
    /// non-guaranteed heads it reads (first-appearance order).
    fn format_pieces(&mut self, parts: &[TemplatePart]) -> (Vec<String>, Vec<String>) {
        let absent_deps = plan::format_absent_deps(self.entry, parts);
        let mut concat: Vec<String> = Vec::new();
        for part in parts {
            match part {
                TemplatePart::Lit(s) => concat.push(format!("{s:?}")),
                TemplatePart::Field(p) => {
                    let t = self.path_type(p);
                    let expr = self.path_expr(p);
                    concat.push(as_template_string(&expr, &t));
                }
                // An op-input placeholder cannot appear in a field template;
                // the frontend rejects it. Render empty defensively.
                TemplatePart::Input(_) => concat.push("\"\"".to_string()),
            }
        }
        (concat, absent_deps)
    }

    /// The `@arg`/`@with` opening shared by the structured and whole-JSON
    /// decodes: an `@arg` value passes typed (returns `None`, having pushed the
    /// assignment onto `out`), otherwise the why-var opens and an optional
    /// `@with` layer wins, and the env source's parts are returned.
    fn decode_opening(
        &mut self,
        field: &EntryField,
        out: &mut String,
    ) -> Option<(String, String, String, String, String)> {
        let dest = self.ident(&field.name);
        if field.sources.iter().any(|s| matches!(s, Source::Arg)) {
            out.push_str(&format!("{dest} = {};", camel(&field.name)));
            return None;
        }
        let why = why_var(&field.name);
        out.push_str(&format!("let {why} = \"no source\";\n"));
        if field.sources.iter().any(|s| matches!(s, Source::With)) {
            let acc = self.with_access(field);
            out.push_str(&format!(
                "if ({acc} !== undefined) {{\n  {dest} = {acc};\n  {why} = \"\";\n}} else {{\n  {why} = \"not configured\";\n}}\n"
            ));
        }
        let Some(Source::Env(name)) = field.sources.iter().find(|s| matches!(s, Source::Env(_)))
        else {
            return None;
        };
        let lookup = self.env_lookup(name);
        let label = self.env_label(name);
        let miss = self.env_miss_reason(name);
        Some((dest, why, lookup, label, miss))
    }

    fn member_arm_body(&mut self, member: &EntryField, value: &ArmValue, dest: &str) -> String {
        match value {
            ArmValue::Lit(v) => format!("{dest} = {};", literal(&member.target, v)),
            ArmValue::Field(path) => {
                let head = path.first().cloned().unwrap_or_default();
                let expr = self.path_expr(path);
                if self.guaranteed(&head) {
                    format!("{dest} = {expr};")
                } else {
                    format!(
                        "if ({head_why} === \"\") {{\n  {dest} = {expr};\n}}",
                        head_why = why_var(&head),
                    )
                }
            }
            ArmValue::Sources(sources) => {
                self.member_chain_body(&plan::arm_sources(member, sources), dest)
            }
        }
    }
}

impl Emitter for Resolver<'_, '_> {
    fn indent_unit(&self) -> &'static str {
        "  "
    }

    fn term(&self) -> &'static str {
        ";"
    }

    fn eq(&self) -> &'static str {
        "==="
    }

    fn neq(&self) -> &'static str {
        "!=="
    }

    fn if_header(&self, cond: &Cond) -> String {
        format!("if ({})", cond.0)
    }

    fn dest(&self, field_name: &str) -> String {
        self.ident(field_name)
    }

    fn member_dest(&self, member_name: &str) -> String {
        format!("composed.{}", field_camel(member_name, self.config))
    }

    fn why_ident(&self, field_name: &str) -> String {
        why_var(field_name)
    }

    fn arg_ident(&self, field: &EntryField) -> String {
        camel(&field.name)
    }

    fn literal_of(&self, target: &Tref, value: &serde_json::Value) -> String {
        literal(target, value)
    }

    fn path_read(&self, path: &[String]) -> String {
        self.path_expr(path)
    }

    fn path_type_of(&self, path: &[String]) -> Tref {
        self.path_type(path)
    }

    fn to_string_expr(&mut self, expr: &str, t: &Tref) -> String {
        as_template_string(expr, t)
    }

    fn env_read_call(&mut self, name_expr: &str) -> String {
        self.helpers.read_env = true;
        format!("readEnv({name_expr})")
    }

    fn why_open(&self, field_name: &str, initial: &str) -> Leaf {
        Leaf(format!("let {} = {initial:?};", why_var(field_name)))
    }

    fn config_open(&mut self, _field: &EntryField, shape: &Shape) -> Leaf {
        Leaf(format!(
            "const composed = {{}} as {};",
            type_ident_from_id(&shape.id)
        ))
    }

    fn bind_expr(&self, source: &[String]) -> String {
        self.path_expr(source)
    }

    /// The `@with` presence step of a non-guaranteed chain, relative to column
    /// zero.
    fn with_step_body(&self, field: &EntryField, dest: &str, why: &str) -> String {
        let acc = self.with_access(field);
        format!(
            "if ({acc} !== undefined) {{\n  {dest} = {acc};\n  {why} = \"\";\n}} else {{\n  {why} = \"not configured\";\n}}"
        )
    }

    /// The env presence step of a non-guaranteed chain: `{ const v = lookup; if
    /// (v !== undefined) { parse } else { miss } }`, prefixed by the name
    /// prereq when the variable name is itself a non-guaranteed sibling.
    fn env_step_body(
        &mut self,
        field: &EntryField,
        name: &EnvName,
        dest: &str,
        why: &str,
    ) -> String {
        let lookup = self.env_lookup(name);
        let label = self.env_label(name);
        let miss = self.env_miss_reason(name);
        let pre = self.env_name_prereq(name, why);
        let parse = self.env_parse(field, dest, &label);
        format!(
            "{pre}{{\n  const v = {lookup};\n  if (v !== undefined) {{\n{parse}\n    {why} = \"\";\n  }} else {{\n    {why} = {miss};\n  }}\n}}",
            parse = nest(&parse, 2),
        )
    }

    /// A guaranteed chain, spelled with a set-flag so every env variable is read
    /// exactly once and `@default` closes the chain. Relative to column zero.
    fn chain_guaranteed(&mut self, field: &EntryField, dest: &str) -> String {
        // A chain that is just the default needs no flag.
        if let [Source::Default(v)] = field.sources.as_slice() {
            return format!("{dest} = {};", literal(&field.target, v));
        }
        let flag = camel(&format!("{}_set", field.name));
        let mut out = format!("let {flag} = false;");
        for source in &field.sources {
            match source {
                Source::With => {
                    let acc = self.with_access(field);
                    out.push_str(&format!(
                        "\nif (!{flag} && {acc} !== undefined) {{\n  {dest} = {acc};\n  {flag} = true;\n}}"
                    ));
                }
                Source::Env(name) => {
                    let lookup = self.env_lookup(name);
                    let label = self.env_label(name);
                    let parse = self.env_parse(field, dest, &label);
                    out.push_str(&format!(
                        "\nif (!{flag}) {{\n  const v = {lookup};\n  if (v !== undefined) {{\n{parse}\n    {flag} = true;\n  }}\n}}",
                        parse = nest(&parse, 2),
                    ));
                }
                Source::Default(v) => {
                    out.push_str(&format!(
                        "\nif (!{flag}) {{\n  {dest} = {};\n}}",
                        literal(&field.target, v),
                    ));
                    return out;
                }
                Source::Arg => {}
            }
        }
        out
    }

    /// `field = match .subject { ... }` lowered to a switch, relative to column
    /// zero.
    fn select_body(&mut self, field: &EntryField, dest: &str) -> String {
        let Some(select) = field.select.clone() else {
            return String::new();
        };
        let guaranteed = self.entry.is_guaranteed(field);
        let why = why_var(&field.name);
        let subject_head = select.subject.first().cloned().unwrap_or_default();
        let subject_expr = self.path_expr(&select.subject);
        let mut arms = String::new();
        let mut saw_wildcard = false;
        for arm in &select.arms {
            let stmts = self.arm_stmts(field, &arm.value, dest, &why, guaranteed);
            match &arm.pattern {
                Some(p) => arms.push_str(&format!(
                    "case {}: {{\n{}\n  break;\n}}\n",
                    pattern_literal(p),
                    nest(&stmts, 1),
                )),
                None => {
                    saw_wildcard = true;
                    arms.push_str(&format!("default: {{\n{}\n  break;\n}}\n", nest(&stmts, 1)));
                }
            }
        }
        if !saw_wildcard {
            // Every enum is open, so an undeclared value can still arrive at
            // run time even with total declared coverage. Failing construction
            // beats freezing a silent zero value into the resolved settings.
            let miss = if guaranteed {
                format!(
                    "throw new Error(`{field}: match on {subject}: unmatched value ${{String({subject_expr})}}`);",
                    field = field.name,
                    subject = subject_head,
                )
            } else {
                format!("{why} = \"match: unmatched value\";")
            };
            arms.push_str(&format!("default: {{\n{}\n  break;\n}}\n", nest(&miss, 1)));
        }
        let switch = format!("switch ({subject_expr}) {{\n{arms}}}");
        let guarded = if self.guaranteed(&subject_head) {
            switch
        } else {
            format!(
                "if ({subj_why} !== \"\") {{\n  {why} = \"{subject_head} <- \" + {subj_why};\n}} else {{\n{sw}\n}}",
                subj_why = why_var(&subject_head),
                sw = nest(&switch, 1),
            )
        };
        if guaranteed {
            guarded
        } else {
            format!("let {why} = \"\";\n{guarded}")
        }
    }

    /// `@format` template plus the `@str::*` pipeline, relative to column zero.
    fn format_body(&mut self, field: &EntryField, dest: &str) -> String {
        let Some(parts) = field.format.clone() else {
            return String::new();
        };
        let (concat, absent_deps) = self.format_pieces(&parts);
        let expr = apply_transforms(concat.join(" + "), &field.transforms, self.helpers);
        let assign = format!("{dest} = {};", cast_string(&field.target, &expr));
        if absent_deps.is_empty() {
            return assign;
        }
        let why = why_var(&field.name);
        let mut chain = format!("let {why} = \"\";\n");
        for (i, dep) in absent_deps.iter().enumerate() {
            chain.push_str(&format!(
                "{}if ({dep_why} !== \"\") {{\n  {why} = \"{dep} <- \" + {dep_why};\n}}",
                if i == 0 { "" } else { " else " },
                dep_why = why_var(dep),
            ));
        }
        chain.push_str(&format!(" else {{\n  {assign}\n}}"));
        chain
    }

    /// The `@str::*` pipeline over an already-resolved destination, or `None`
    /// when the field declares no transforms.
    fn transforms_body(&mut self, field: &EntryField, dest: &str) -> Option<String> {
        if field.transforms.is_empty() {
            return None;
        }
        let expr = as_template_string(dest, &field.target);
        let expr = apply_transforms(expr, &field.transforms, self.helpers);
        Some(format!("{dest} = {};", cast_string(&field.target, &expr)))
    }

    /// A structured source decoded strictly (required members first, then
    /// unknown fields, then per-member scalar type checks, mirroring the Go
    /// order), plus declared validation. Relative to column zero.
    fn structured_body(&mut self, field: &EntryField, shape: &Shape) -> String {
        let mut out = String::new();
        let Some((dest, why, lookup, label, miss)) = self.decode_opening(field, &mut out) else {
            return out;
        };
        let ty = type_ident_from_id(&shape.id);
        let mut known = Vec::new();
        let mut required_checks = String::new();
        let mut type_checks = String::new();
        if let ShapeKind::Structure { members, .. } = &shape.kind {
            for m in members {
                known.push(format!("{:?}", m.name));
                if m.required {
                    // An explicit null is as absent as a missing key, the same
                    // rule the Go probe applies.
                    required_checks.push_str(&format!(
                        "if (!({name:?} in parsed) || record[{name:?}] === null) {{\n  throw new Error(`${{{label}}}: missing field {name}`);\n}}\n",
                        name = m.name,
                    ));
                }
                // Scalar wire-type checks keep the strictness on par with the Go
                // decoder (which is typed); containers and refs decode as the
                // wire codec always has.
                let expected = match &m.target {
                    Tref::Prim(
                        Prim::String
                        | Prim::Uuid
                        | Prim::Timestamp
                        | Prim::Date
                        | Prim::Duration
                        | Prim::Bytes
                        | Prim::I64
                        | Prim::U64,
                    ) => Some(("string", "a string")),
                    Tref::Prim(
                        Prim::I8
                        | Prim::I16
                        | Prim::I32
                        | Prim::U8
                        | Prim::U16
                        | Prim::U32
                        | Prim::Float,
                    ) => Some(("number", "a number")),
                    Tref::Prim(Prim::Bool) => Some(("boolean", "a boolean")),
                    _ => None,
                };
                if let Some((ts_typeof, describe)) = expected {
                    let guard = if m.required {
                        String::new()
                    } else {
                        format!(
                            "record[{name:?}] !== undefined && record[{name:?}] !== null && ",
                            name = m.name
                        )
                    };
                    type_checks.push_str(&format!(
                        "if ({guard}{present}typeof record[{name:?}] !== {ts_typeof:?}) {{\n  throw new Error(`${{{label}}}: field {name} must be {describe}`);\n}}\n",
                        present = if m.required {
                            format!("{name:?} in parsed && ", name = m.name)
                        } else {
                            String::new()
                        },
                        name = m.name,
                    ));
                }
            }
        }
        let validate = if validation::shape_has_checks(shape) {
            let en = error_names();
            format!(
                "const vs = validate{ty}(decoded);\nif (vs.length > 0) {{\n  throw new {validation}(vs);\n}}\n",
                validation = en.validation,
            )
        } else {
            String::new()
        };
        let inner = format!(
            "const raw = {lookup};\nif (raw !== undefined) {{\n\
             \x20 let parsed: unknown;\n\
             \x20 try {{\n    parsed = JSON.parse(raw);\n  }} catch (e) {{\n    throw new Error(`${{{label}}}: ${{String(e)}}`);\n  }}\n\
             \x20 if (typeof parsed !== \"object\" || parsed === null || Array.isArray(parsed)) {{\n    throw new Error(`${{{label}}}: expected an object`);\n  }}\n\
             \x20 const record = parsed as Record<string, unknown>;\n\
             {required}\
             \x20 for (const key of Object.keys(parsed)) {{\n    if (![{known}].includes(key)) {{\n      throw new Error(`${{{label}}}: unknown field ${{key}}`);\n    }}\n  }}\n\
             {types}\
             \x20 const decoded = decode{ty}(parsed);\n\
             {validate}\
             \x20 {dest} = decoded;\n\
             \x20 {why} = \"\";\n\
             }} else {{\n  {why} = {miss};\n}}",
            known = known.join(", "),
            required = block1(&required_checks),
            types = block1(&type_checks),
            validate = block1(&validate),
        );
        out.push_str(&format!("if ({why} !== \"\") {{\n{}\n}}", nest(&inner, 1)));
        out
    }

    /// A map/list field decoded as JSON whole, through the wire codecs so an
    /// i64 map or union field lands typed. Relative to column zero.
    fn json_body(&mut self, field: &EntryField) -> String {
        let mut out = String::new();
        let Some((dest, why, lookup, label, miss)) = self.decode_opening(field, &mut out) else {
            return out;
        };
        let ty = ts_type(&field.target);
        let decode =
            crate::codegen::targets::typescript::codecs::decode_expr("parsed", &field.target);
        // Container and element checks keep the boundary as strict as Go's typed
        // unmarshal: the same env value must construct in both targets.
        let checks = block1(&json_shape_checks(&field.target, &label));
        let inner = format!(
            "const raw = {lookup};\nif (raw !== undefined) {{\n  let parsed: any;\n  try {{\n    parsed = JSON.parse(raw);\n  }} catch (e) {{\n    throw new Error(`${{{label}}}: ${{String(e)}}`);\n  }}\n{checks}  {dest} = {decode} as {ty};\n  {why} = \"\";\n}} else {{\n  {why} = {miss};\n}}"
        );
        out.push_str(&format!("if ({why} !== \"\") {{\n{}\n}}", nest(&inner, 1)));
        out
    }

    /// A member match, lowered without a why-var. Relative to column zero.
    fn member_select_body(&mut self, member: &EntryField, dest: &str) -> String {
        let Some(select) = member.select.clone() else {
            return String::new();
        };
        let subject_head = select.subject.first().cloned().unwrap_or_default();
        let subject_expr = self.path_expr(&select.subject);
        let mut arms = String::new();
        for arm in &select.arms {
            let stmts = self.member_arm_body(member, &arm.value, dest);
            match &arm.pattern {
                Some(p) => arms.push_str(&format!(
                    "case {}: {{\n{}\n  break;\n}}\n",
                    pattern_literal(p),
                    nest(&stmts, 1),
                )),
                None => arms.push_str(&format!("default: {{\n{}\n  break;\n}}\n", nest(&stmts, 1))),
            }
        }
        let switch = format!("switch ({subject_expr}) {{\n{arms}}}");
        if self.guaranteed(&subject_head) {
            switch
        } else {
            format!(
                "if ({subj_why} === \"\") {{\n{}\n}}",
                nest(&switch, 1),
                subj_why = why_var(&subject_head),
            )
        }
    }

    /// A config member's own resolution (no reason tracking: an absent member
    /// keeps its zero value), relative to column zero.
    fn member_chain_body(&mut self, stub: &EntryField, dest: &str) -> String {
        let guaranteed = stub.sources.iter().any(|s| matches!(s, Source::Default(_)));
        if guaranteed {
            return self.chain_guaranteed(stub, dest);
        }
        let mut out = String::new();
        for source in &stub.sources {
            if let Source::Env(name) = source {
                let lookup = self.env_lookup(name);
                let label = self.env_label(name);
                let parse = self.env_parse(stub, dest, &label);
                out.push_str(&format!(
                    "{{\n  const v = {lookup};\n  if (v !== undefined) {{\n{}\n  }}\n}}\n",
                    nest(&parse, 2),
                ));
            }
        }
        out.trim_end_matches('\n').to_string()
    }
}

/// Indent every non-empty line of `s` by `n` two-space units, dropping any
/// trailing newline so callers control the separator.
fn nest(s: &str, n: usize) -> String {
    let pad = "  ".repeat(n);
    s.trim_end_matches('\n')
        .split('\n')
        .map(|l| {
            if l.is_empty() {
                String::new()
            } else {
                format!("{pad}{l}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// A structured-decode sub-block indented one level with a trailing newline, or
/// empty when the block is empty (so an absent check adds no blank line).
fn block1(s: &str) -> String {
    if s.is_empty() {
        String::new()
    } else {
        format!("{}\n", nest(s, 1))
    }
}
