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
    fn guaranteed(&self, name: &str) -> bool {
        self.entry.field_guaranteed(name)
    }

    fn with_access(&self, field: &EntryField) -> String {
        format!("config.{}", field_camel(&field.name, self.config))
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
    fn decode_opening(&mut self, field: &EntryField, out: &mut String) -> Option<(String, String)> {
        let dest = self.ident(&field.name);
        if field.sources.iter().any(|s| matches!(s, Source::Arg)) {
            out.push_str(&format!("{dest} = {};", camel(&field.name)));
            return None;
        }
        let why = why_var(&field.name);
        out.push_str(&format!("let {why} = \"no source\";\n"));
        Some((dest, why))
    }

    /// Lay out a decode field's source cascade below the why-var opening: the
    /// first source runs unconditionally, every later one only while the why-var
    /// is still set (a first-present-wins fallback). Each `@env` step is built by
    /// `env_block` from its own `(lookup, label, miss, prereq)`; `@with` and
    /// `@default` are spelled inline.
    fn decode_cascade(
        &mut self,
        field: &EntryField,
        dest: &str,
        why: &str,
        mut env_block: impl FnMut(&mut Self, &str, &str, &str, &str) -> String,
    ) -> String {
        let mut steps: Vec<String> = Vec::new();
        for source in &field.sources {
            match source {
                Source::With => steps.push(self.with_step_body(field, dest, why)),
                Source::Env(name) => {
                    let lookup = self.env_lookup(name);
                    let label = self.env_label(name);
                    let miss = self.env_miss_reason(name);
                    let pre = self.env_name_prereq(name, why);
                    steps.push(env_block(self, &lookup, &label, &miss, &pre));
                }
                Source::Default(v) => steps.push(format!(
                    "{dest} = {};\n{why} = \"\";",
                    literal(&field.target, v)
                )),
                Source::Arg => {}
            }
        }
        steps
            .iter()
            .enumerate()
            .map(|(i, step)| {
                if i == 0 {
                    step.clone()
                } else {
                    format!("if ({why} !== \"\") {{\n{}\n}}", nest(step, 1))
                }
            })
            .collect::<Vec<_>>()
            .join("\n")
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

    fn member_dest(&self, member_name: &str) -> String {
        format!("composed.{}", field_camel(member_name, self.config))
    }

    fn literal_of(&self, target: &Tref, value: &serde_json::Value) -> String {
        literal(target, value)
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

    fn switch_header(&self, subject: &str) -> String {
        format!("switch ({subject})")
    }

    fn case_open(&self, pattern: &str) -> String {
        format!("case {pattern}: {{")
    }

    fn default_open(&self) -> String {
        "default: {".to_string()
    }

    fn case_tail(&self) -> Option<&'static str> {
        Some("break;")
    }

    fn case_close(&self) -> Option<&'static str> {
        Some("}")
    }

    fn pattern_lit(&self, pattern: &serde_json::Value) -> String {
        pattern_literal(pattern)
    }

    /// Every enum is open, so an undeclared value can still arrive at run time
    /// even with total declared coverage. Failing construction beats freezing a
    /// silent zero value into the resolved settings.
    fn select_miss(
        &mut self,
        field: &EntryField,
        subject_head: &str,
        subject_expr: &str,
        guaranteed: bool,
    ) -> Leaf {
        if guaranteed {
            Leaf(format!(
                "throw new Error(`{field}: match on {subject}: unmatched value ${{String({subject_expr})}}`);",
                field = field.name,
                subject = subject_head,
            ))
        } else {
            Leaf(format!(
                "{} = \"match: unmatched value\";",
                why_var(&field.name)
            ))
        }
    }

    /// The `@format` template concatenation cast to the field type, with the
    /// `@str::*` pipeline folded in. Relative to column zero.
    fn format_assign(&mut self, field: &EntryField, dest: &str) -> String {
        let Some(parts) = field.format.clone() else {
            return String::new();
        };
        let (concat, _) = self.format_pieces(&parts);
        let expr = apply_transforms(concat.join(" + "), &field.transforms, self.helpers);
        format!("{dest} = {};", cast_string(&field.target, &expr))
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
        let Some((dest, why)) = self.decode_opening(field, &mut out) else {
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
                        "if (!({name:?} in parsed) || record[{name:?}] === null) {{\n  throw new Error(`${{__LABEL__}}: missing field {name}`);\n}}\n",
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
                        "if ({guard}{present}typeof record[{name:?}] !== {ts_typeof:?}) {{\n  throw new Error(`${{__LABEL__}}: field {name} must be {describe}`);\n}}\n",
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
        let known = known.join(", ");
        let required_tpl = block1(&required_checks);
        let types_tpl = block1(&type_checks);
        let validate = block1(&validate);
        let (dc, wc) = (dest.clone(), why.clone());
        let body = self.decode_cascade(field, &dest, &why, move |_, lookup, label, miss, pre| {
            // The required/type guards carry a `__LABEL__` slot so each source in
            // the cascade renders them against its own env label.
            let required = required_tpl.replace("__LABEL__", label);
            let types = types_tpl.replace("__LABEL__", label);
            format!(
                "{pre}const raw = {lookup};\nif (raw !== undefined) {{\n\
                 \x20 let parsed: unknown;\n\
                 \x20 try {{\n    parsed = JSON.parse(raw);\n  }} catch (e) {{\n    throw new Error(`${{{label}}}: ${{String(e)}}`);\n  }}\n\
                 \x20 if (typeof parsed !== \"object\" || parsed === null || Array.isArray(parsed)) {{\n    throw new Error(`${{{label}}}: expected an object`);\n  }}\n\
                 \x20 const record = parsed as Record<string, unknown>;\n\
                 {required}\
                 \x20 for (const key of Object.keys(parsed)) {{\n    if (![{known}].includes(key)) {{\n      throw new Error(`${{{label}}}: unknown field ${{key}}`);\n    }}\n  }}\n\
                 {types}\
                 \x20 const decoded = decode{ty}(parsed);\n\
                 {validate}\
                 \x20 {dc} = decoded;\n\
                 \x20 {wc} = \"\";\n\
                 }} else {{\n  {wc} = {miss};\n}}"
            )
        });
        out.push_str(&body);
        out
    }

    /// A map/list field decoded as JSON whole, through the wire codecs so an
    /// i64 map or union field lands typed. Relative to column zero.
    fn json_body(&mut self, field: &EntryField) -> String {
        let mut out = String::new();
        let Some((dest, why)) = self.decode_opening(field, &mut out) else {
            return out;
        };
        let ty = ts_type(&field.target);
        let decode =
            crate::codegen::targets::typescript::codecs::decode_expr("parsed", &field.target);
        let target = field.target.clone();
        let (dc, wc) = (dest.clone(), why.clone());
        let body = self.decode_cascade(field, &dest, &why, move |_, lookup, label, miss, pre| {
            // Container and element checks keep the boundary as strict as Go's
            // typed unmarshal: the same env value must construct in both targets.
            let checks = block1(&json_shape_checks(&target, label));
            format!(
                "{pre}const raw = {lookup};\nif (raw !== undefined) {{\n  let parsed: any;\n  try {{\n    parsed = JSON.parse(raw);\n  }} catch (e) {{\n    throw new Error(`${{{label}}}: ${{String(e)}}`);\n  }}\n{checks}  {dc} = {decode} as {ty};\n  {wc} = \"\";\n}} else {{\n  {wc} = {miss};\n}}"
            )
        });
        out.push_str(&body);
        out
    }

    fn require_member(&mut self, head: &str, member: &str, leaf: &Tref, name: &str) -> String {
        format!(
            "if ((s.{head}.{member} ?? {zero}) === {zero}) {{\n  throw new Error(\"{name}: no value\");\n}}",
            head = field_camel(head, self.config),
            member = field_camel(member, self.config),
            zero = cast_string(leaf, "\"\""),
        )
    }

    fn require_string(&mut self, head: &str, target: &Tref) -> String {
        format!(
            "if (s.{ident} === {zero}) {{\n  throw new Error(\"{name} <- \" + ({why} || \"no value\"));\n}}",
            ident = field_camel(head, self.config),
            zero = cast_string(target, "\"\""),
            why = why_var(head),
            name = head,
        )
    }

    fn require_bytes(&mut self, head: &str) -> String {
        format!(
            "if (s.{ident}.length === 0) {{\n  throw new Error(\"{name} <- \" + ({why} || \"no value\"));\n}}",
            ident = field_camel(head, self.config),
            why = why_var(head),
            name = head,
        )
    }

    fn require_numeric(&mut self, head: &str, target: &Tref) -> String {
        let zero = if matches!(target, Tref::Prim(Prim::I64 | Prim::U64)) {
            "0n"
        } else {
            "0"
        };
        format!(
            "if ({why} !== \"\" && s.{ident} === {zero}) {{\n  throw new Error(\"{name} <- \" + {why});\n}}",
            ident = field_camel(head, self.config),
            why = why_var(head),
            name = head,
        )
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
