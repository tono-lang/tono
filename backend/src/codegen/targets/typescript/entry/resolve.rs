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
/// `resolve_fns` collects each top-level field's standalone resolver function
/// (see [`Emitter::resolve_fn_call`]); the caller flushes it into the entry's
/// own decls once every field is built. `multi` names those functions with
/// the entry's own prefix in a multi-entry module.
pub(super) struct Resolver<'a, 'b> {
    pub(super) entry: &'a EntryModel<'a>,
    pub(super) module: &'a Module,
    pub(super) config: &'a CasingConfig,
    pub(super) helpers: &'b mut Helpers,
    pub(super) body: &'b mut String,
    pub(super) resolve_fns: &'b mut Vec<Decl>,
    pub(super) multi: bool,
}

impl Resolver<'_, '_> {
    fn with_access(&self, field: &EntryField) -> String {
        format!(
            "config.{}",
            field_camel_ren(
                &field.name,
                rename_of(&field.traits, LANG).as_deref(),
                self.config
            )
        )
    }

    /// The name of a top-level field's standalone resolver function,
    /// entry-prefixed only in a multi-entry module (matching every other
    /// per-entry companion name).
    fn resolve_fn_name(&self, field: &EntryField) -> String {
        // "Setting" disambiguates from the shared per-op helpers named after
        // a field-shaped concept directly (`resolveMaxRetries`,
        // `resolveTimeoutMs`): a field named `max_retries` would otherwise
        // collide with the unrelated retry-count helper.
        if self.multi {
            format!(
                "resolveSetting{}{}",
                pascal(self.entry.name),
                pascal(&field.name)
            )
        } else {
            format!("resolveSetting{}", pascal(&field.name))
        }
    }

    /// Parse a raw env string `v` into the destination, by the declared type; a
    /// parse failure fails construction naming the variable and the type. The
    /// body sits inside `if (v !== undefined) { .. }`, relative to column zero.
    fn env_parse(&mut self, field: &EntryField, dest: &str, label: &str) -> String {
        let t = &field.target;
        match t {
            Tref::Prim(Prim::Bool) => {
                let fail = config_error(&format!(
                    "`${{{label}}}: invalid bool ${{JSON.stringify(v)}} (want true/false/1/0)`"
                ));
                format!(
                    "if (v === \"true\" || v === \"1\") {{\n  {dest} = true;\n}} else if (v === \"false\" || v === \"0\") {{\n  {dest} = false;\n}} else {{\n  {fail}\n}}"
                )
            }
            Tref::Prim(
                p @ (Prim::I8 | Prim::I16 | Prim::I32 | Prim::U8 | Prim::U16 | Prim::U32),
            ) => {
                // Decimal digits only plus the type's own range, matching the Go
                // boundary (strconv with an explicit bit size, which takes a
                // sign only for the signed types).
                let (min, max) = int_bounds(p);
                let regex = if matches!(p, Prim::I8 | Prim::I16 | Prim::I32) {
                    "/^[+-]?[0-9]+$/"
                } else {
                    "/^[0-9]+$/"
                };
                let fail = config_error(&format!(
                    "`${{{label}}}: invalid {prim} ${{JSON.stringify(v)}}`",
                    prim = prim_name(p),
                ));
                format!(
                    "{{\n  if (!{regex}.test(v)) {{\n    {fail}\n  }}\n  const n = Number(v);\n  if (!Number.isInteger(n) || n < {min} || n > {max}) {{\n    {fail}\n  }}\n  {dest} = n;\n}}",
                )
            }
            Tref::Prim(p @ (Prim::I64 | Prim::U64)) => {
                let (regex, min, max) = if matches!(p, Prim::I64) {
                    (
                        "/^[+-]?[0-9]+$/",
                        "-9223372036854775808n",
                        "9223372036854775807n",
                    )
                } else {
                    // ParseUint takes no sign at all, so neither does this.
                    ("/^[0-9]+$/", "0n", "18446744073709551615n")
                };
                let fail = config_error(&format!(
                    "`${{{label}}}: invalid {prim} ${{JSON.stringify(v)}}`",
                    prim = prim_name(p),
                ));
                format!(
                    "{{\n  if (!{regex}.test(v)) {{\n    {fail}\n  }}\n  const n = BigInt(v.startsWith(\"+\") ? v.slice(1) : v);\n  if (n < {min} || n > {max}) {{\n    {fail}\n  }}\n  {dest} = n;\n}}",
                )
            }
            Tref::Prim(Prim::Float) => {
                let fail = config_error(&format!(
                    "`${{{label}}}: invalid float ${{JSON.stringify(v)}}`"
                ));
                // Decimal notation only: bare Number() also accepts hex and
                // Infinity spellings the Go boundary rejects.
                format!(
                    "{{\n  if (!/^[+-]?(\\d+(\\.\\d*)?|\\.\\d+)([eE][+-]?\\d+)?$/.test(v)) {{\n    {fail}\n  }}\n  const n = Number(v);\n  if (!Number.isFinite(n)) {{\n    {fail}\n  }}\n  {dest} = n;\n}}"
                )
            }
            Tref::Prim(Prim::Bytes) => {
                let fail = config_error(&format!(
                    "`${{{label}}}: invalid base64 ${{JSON.stringify(v)}}`"
                ));
                // The env boundary carries bytes the same way the wire does:
                // base64 text.
                format!("try {{\n  {dest} = decodeBytes(v);\n}} catch {{\n  {fail}\n}}")
            }
            Tref::Prim(Prim::Duration) => {
                self.helpers.duration_ms = true;
                let fail = config_error(&format!(
                    "`${{{label}}}: invalid duration ${{JSON.stringify(v)}}`"
                ));
                format!(
                    "try {{\n  durationToMs(v);\n}} catch {{\n  {fail}\n}}\n{dest} = v as Duration;"
                )
            }
            _ => format!("{dest} = {};", cast_string(t, "v")),
        }
    }

    /// The template split: each part as a TS string expression, plus the
    /// non-guaranteed heads it reads (first-appearance order).
    fn format_pieces(&mut self, parts: &[TemplatePart]) -> (Vec<String>, Vec<String>) {
        plan::format_pieces(self.entry, parts, |p| {
            let t = self.path_type(p);
            let expr = self.path_expr(p);
            as_template_string(&expr, &t)
        })
    }

    /// Lay out a decode field's source cascade below the error-var opening.
    /// Each `@env` step is built by `env_block` from its own `(lookup, label,
    /// miss, prereq)`, computed here before delegating the shared ordering and
    /// wrapping to `plan::decode_cascade`.
    fn decode_cascade(
        &mut self,
        field: &EntryField,
        dest: &str,
        err: &str,
        mut env_block: impl FnMut(&mut Self, &str, &str, &str, &str) -> String,
    ) -> String {
        plan::decode_cascade(
            self,
            field,
            dest,
            err,
            |s| s.with_step_body(field, dest, err),
            |s, name| {
                let (lookup, label, miss, pre) = plan::env_parts(s, name, err);
                env_block(s, &lookup, &label, &miss, &pre)
            },
            literal,
        )
    }

    fn chain_guaranteed_from(&mut self, field: &EntryField, dest: &str, idx: usize) -> String {
        match field.sources.get(idx) {
            None => String::new(),
            Some(Source::Arg) => self.chain_guaranteed_from(field, dest, idx + 1),
            Some(Source::With) => {
                let acc = self.with_access(field);
                let rest = self.chain_guaranteed_from(field, dest, idx + 1);
                format!(
                    "if ({acc} !== undefined) {{\n  {dest} = {acc};\n}} else {{\n{}\n}}",
                    plan::nest("  ", &rest, 1),
                )
            }
            Some(Source::Env(name)) => {
                let lookup = self.env_lookup(name);
                let label = self.env_label(name);
                let parse = self.env_parse(field, dest, &label);
                let rest = self.chain_guaranteed_from(field, dest, idx + 1);
                format!(
                    "const v = {lookup};\nif (v !== undefined) {{\n{parse}\n}} else {{\n{rest}\n}}",
                    parse = plan::nest("  ", &parse, 1),
                    rest = plan::nest("  ", &rest, 1),
                )
            }
            Some(Source::Default(v)) => format!("{dest} = {};", literal(&field.target, v)),
        }
    }
}

impl Emitter for Resolver<'_, '_> {
    fn indent_unit(&self) -> &'static str {
        "  "
    }

    fn lang(&self) -> &'static str {
        LANG
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
        format!(
            "s.{}",
            field_camel_ren(
                name,
                self.entry.field_rename(name, LANG).as_deref(),
                self.config
            )
        )
    }

    fn path_expr(&self, path: &[String]) -> String {
        field_path_expr(self.entry, self.config, path, "s")
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

    fn err_open(&self, field_name: &str) -> Leaf {
        Leaf(format!(
            "let {}: ConfigError | undefined;",
            err_var(field_name)
        ))
    }

    fn absent_literal(&self) -> &'static str {
        "undefined"
    }

    fn cond_err_present(&self, field_name: &str) -> Cond {
        Cond(format!("{} !== undefined", self.err_ident(field_name)))
    }

    fn cond_err_absent(&self, field_name: &str) -> Cond {
        Cond(format!("{} === undefined", self.err_ident(field_name)))
    }

    fn wrap_error_expr(&self, head: &str, head_err: &str) -> String {
        let message = format!("`{head} <- ${{{head_err}.message}}`");
        self.config_error_expr(&message, Some(head_err))
    }

    fn field_guaranteed(&self, name: &str) -> bool {
        self.entry.field_guaranteed(name)
    }

    fn config_error_expr(&self, message_expr: &str, cause: Option<&str>) -> String {
        match cause {
            Some(cause) => format!("new {}({message_expr}, {cause})", error_names().config),
            None => format!("new {}({message_expr})", error_names().config),
        }
    }

    fn call_assign(
        &mut self,
        field: &EntryField,
        call: &crate::ir::EntryCall,
        dest: &str,
    ) -> String {
        super::ext_call::call_assign(self, field, call, dest)
    }

    fn config_open(&mut self, _field: &EntryField, shape: &Shape) -> Leaf {
        Leaf(format!(
            "const composed = {{}} as {};",
            type_ident_from_id(&shape.id)
        ))
    }

    /// The `@with` presence step of a non-guaranteed chain, relative to column
    /// zero.
    fn with_step_body(&self, field: &EntryField, dest: &str, err: &str) -> String {
        let acc = self.with_access(field);
        format!(
            "if ({acc} !== undefined) {{\n  {dest} = {acc};\n  {err} = undefined;\n}} else {{\n  {err} = {miss};\n}}",
            miss = self.config_error_expr("\"not configured\"", None),
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
        err: &str,
    ) -> String {
        let (lookup, label, miss, pre) = plan::env_parts(self, name, err);
        let parse = self.env_parse(field, dest, &label);
        format!(
            "{pre}{{\n  const v = {lookup};\n  if (v !== undefined) {{\n{parse}\n    {err} = undefined;\n  }} else {{\n    {err} = {miss};\n  }}\n}}",
            parse = plan::nest("  ", &parse, 2),
        )
    }

    /// A guaranteed chain matching Go/Rust's if/else-if cascade: each
    /// source's `else` only runs once every higher-priority source already
    /// missed, so a lower-priority `@env` is never even parsed (its failure
    /// mode never fires) while an earlier source still wins. A `@with`
    /// source stays flat (`config.field` narrows across the condition and
    /// the body: the same simple property read, no intervening call), but an
    /// `@env` source nests: TypeScript does not narrow `readEnv(...)` a
    /// second call the way it narrows a variable, so `const v = readEnv(...)`
    /// has to sit inside the `else` its own condition opens, not read
    /// `readEnv` again to reuse the outer flat-chain shape. A set-flag
    /// sequence was the other way to avoid a second call, but it evaluates
    /// every source regardless of priority and only gates the final
    /// assignment, so a malformed but shadowed `@env` value would fail
    /// construction it does not fail today. Relative to column zero.
    fn chain_guaranteed(&mut self, field: &EntryField, dest: &str) -> String {
        self.chain_guaranteed_from(field, dest, 0)
    }

    /// A top-level guaranteed field's chain as a standalone function: each
    /// source is an unconditional early-return guard in priority order, so
    /// there is no `else` to spell (a later guard is simply never reached
    /// once an earlier one returns) and no set-flag either. This also
    /// sidesteps the narrowing limitation `chain_guaranteed_from` works around
    /// for TypeScript's inline `@env` nesting: `readEnv` is bound to a
    /// `const` and returned immediately, never read a second time. `@with`
    /// reads off a function parameter, not the constructor's `config`
    /// argument, so the function has no free variable tying it to one call
    /// site.
    fn resolve_fn_call(&mut self, field: &EntryField, dest: &str) -> String {
        let name = self.resolve_fn_name(field);
        let has_with = field.sources.iter().any(|s| matches!(s, Source::With));
        let ty = ts_type(&field.target);
        let mut body = String::new();
        for source in &field.sources {
            match source {
                Source::With => {
                    body.push_str("if (withValue !== undefined) return withValue;\n");
                }
                Source::Env(name) => {
                    let lookup = self.env_lookup(name);
                    let label = self.env_label(name);
                    let parse = self.env_parse(field, "parsed", &label);
                    body.push_str(&format!(
                        "{{\n  const v = {lookup};\n  if (v !== undefined) {{\n    let parsed: {ty};\n{parse}\n    return parsed;\n  }}\n}}\n",
                        parse = plan::nest("  ", &parse, 2),
                    ));
                }
                Source::Default(v) => {
                    body.push_str(&format!("return {};\n", literal(&field.target, v)));
                }
                Source::Arg => {}
            }
        }
        let param = if has_with {
            format!("withValue: {ty} | undefined")
        } else {
            String::new()
        };
        self.resolve_fns.push(Decl::raw(format!(
            "function {name}({param}): {ty} {{\n{body}}}",
        )));
        let arg = if has_with {
            self.with_access(field)
        } else {
            String::new()
        };
        format!("{dest} = {name}({arg});")
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
            Leaf(config_error(&format!(
                "`{field}: match on {subject}: unmatched value ${{String({subject_expr})}}`",
                field = field.name,
                subject = subject_head,
            )))
        } else {
            Leaf(format!(
                "{} = {};",
                err_var(&field.name),
                self.config_error_expr("\"match: unmatched value\"", None),
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
        let Some((dest, err)) = plan::decode_opening(self, field, &mut out) else {
            return out;
        };
        let ty = type_ident_from_id(&shape.id);
        let mut known = Vec::new();
        let mut required_checks = String::new();
        let mut type_checks = String::new();
        if let ShapeKind::Structure { members, .. } = &shape.kind {
            for m in members {
                // Every check reads the wire key (the @wire override the codec
                // serializes under), not the in-code member name; `parsed` is the
                // raw wire object.
                let name = wire_key(m);
                known.push(format!("{name:?}"));
                if m.required {
                    // An explicit null is as absent as a missing key, the same
                    // rule the Go probe applies.
                    let fail = config_error(&format!("`${{__LABEL__}}: missing field {name}`"));
                    required_checks.push_str(&format!(
                        "if (!({name:?} in parsed) || record[{name:?}] === null) {{\n  {fail}\n}}\n",
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
                        format!("record[{name:?}] !== undefined && record[{name:?}] !== null && ")
                    };
                    let fail = config_error(&format!(
                        "`${{__LABEL__}}: field {name} must be {describe}`"
                    ));
                    type_checks.push_str(&format!(
                        "if ({guard}{present}typeof record[{name:?}] !== {ts_typeof:?}) {{\n  {fail}\n}}\n",
                        present = if m.required {
                            format!("{name:?} in parsed && ")
                        } else {
                            String::new()
                        },
                    ));
                }
            }
        }
        let validate = if validation::shape_has_checks(shape) {
            format!(
                "const invalid = validate{ty}(decoded);\nif (invalid) {{\n  throw invalid;\n}}\n"
            )
        } else {
            String::new()
        };
        let known = known.join(", ");
        let required_tpl = block1(&required_checks);
        let types_tpl = block1(&type_checks);
        let validate = block1(&validate);
        let (dc, ec) = (dest.clone(), err.clone());
        let body = self.decode_cascade(field, &dest, &err, move |_, lookup, label, miss, pre| {
            // The required/type guards carry a `__LABEL__` slot so each source in
            // the cascade renders them against its own env label.
            let required = required_tpl.replace("__LABEL__", label);
            let types = types_tpl.replace("__LABEL__", label);
            let fail_parse = config_error(&format!("`${{{label}}}: ${{String(e)}}`"));
            let fail_object = config_error(&format!("`${{{label}}}: expected an object`"));
            let fail_unknown = config_error(&format!("`${{{label}}}: unknown field ${{key}}`"));
            format!(
                "{pre}const raw = {lookup};\nif (raw !== undefined) {{\n\
                 \x20 let parsed: unknown;\n\
                 \x20 try {{\n    parsed = JSON.parse(raw);\n  }} catch (e) {{\n    {fail_parse}\n  }}\n\
                 \x20 if (typeof parsed !== \"object\" || parsed === null || Array.isArray(parsed)) {{\n    {fail_object}\n  }}\n\
                 \x20 const record = parsed as Record<string, unknown>;\n\
                 {required}\
                 \x20 for (const key of Object.keys(parsed)) {{\n    if (![{known}].includes(key)) {{\n      {fail_unknown}\n    }}\n  }}\n\
                 {types}\
                 \x20 const decoded = decode{ty}(parsed);\n\
                 {validate}\
                 \x20 {dc} = decoded;\n\
                 \x20 {ec} = undefined;\n\
                 }} else {{\n  {ec} = {miss};\n}}"
            )
        });
        out.push_str(&body);
        out
    }

    /// A map/list field decoded as JSON whole, through the wire codecs so an
    /// i64 map or union field lands typed. Relative to column zero.
    fn json_body(&mut self, field: &EntryField) -> String {
        let mut out = String::new();
        let Some((dest, err)) = plan::decode_opening(self, field, &mut out) else {
            return out;
        };
        let ty = ts_type(&field.target);
        let decode =
            crate::codegen::targets::typescript::codecs::decode_expr("parsed", &field.target);
        let target = field.target.clone();
        let (dc, ec) = (dest.clone(), err.clone());
        let body = self.decode_cascade(field, &dest, &err, move |_, lookup, label, miss, pre| {
            // Container and element checks keep the boundary as strict as Go's
            // typed unmarshal: the same env value must construct in both targets.
            let checks = block1(&json_shape_checks(&target, label));
            let fail_parse = config_error(&format!("`${{{label}}}: ${{String(e)}}`"));
            format!(
                "{pre}const raw = {lookup};\nif (raw !== undefined) {{\n  let parsed: any;\n  try {{\n    parsed = JSON.parse(raw);\n  }} catch (e) {{\n    {fail_parse}\n  }}\n{checks}  {dc} = {decode} as {ty};\n  {ec} = undefined;\n}} else {{\n  {ec} = {miss};\n}}"
            )
        });
        out.push_str(&body);
        out
    }

    fn require_member(&mut self, head: &str, member: &str, leaf: &Tref, name: &str) -> String {
        format!(
            "if ((s.{head}.{member} ?? {zero}) === {zero}) {{\n  throw new {config}(\"{name}: no value\");\n}}",
            head = field_camel_ren(head, self.entry.field_rename(head, LANG).as_deref(), self.config),
            member = field_camel(member, self.config),
            zero = cast_string(leaf, "\"\""),
            config = error_names().config,
        )
    }

    fn require_member_deferred(&mut self, name: &str, err: &str) -> String {
        let ident = err_var(err);
        format!(
            "if ({ident} !== undefined) {{\n  throw new {config}(\"{name} <- \" + {ident}.message, {ident});\n}}",
            config = error_names().config,
        )
    }

    fn require_string(&mut self, head: &str, target: &Tref) -> String {
        format!(
            "if (s.{ident} === {zero}) {{\n  throw new {config}(\"{name} <- \" + ({err} ? {err}.message : \"no value\"), {err});\n}}",
            ident = field_camel_ren(head, self.entry.field_rename(head, LANG).as_deref(), self.config),
            zero = cast_string(target, "\"\""),
            err = err_var(head),
            name = head,
            config = error_names().config,
        )
    }

    fn require_bytes(&mut self, head: &str) -> String {
        format!(
            "if (s.{ident}.length === 0) {{\n  throw new {config}(\"{name} <- \" + ({err} ? {err}.message : \"no value\"), {err});\n}}",
            ident = field_camel_ren(head, self.entry.field_rename(head, LANG).as_deref(), self.config),
            err = err_var(head),
            name = head,
            config = error_names().config,
        )
    }

    fn require_numeric(&mut self, head: &str, target: &Tref) -> String {
        let zero = if matches!(target, Tref::Prim(Prim::I64 | Prim::U64)) {
            "0n"
        } else {
            "0"
        };
        format!(
            "if ({err} !== undefined && s.{ident} === {zero}) {{\n  throw new {config}(\"{name} <- \" + {err}.message, {err});\n}}",
            ident = field_camel_ren(head, self.entry.field_rename(head, LANG).as_deref(), self.config),
            err = err_var(head),
            name = head,
            config = error_names().config,
        )
    }
}

/// A structured-decode sub-block indented one level with a trailing newline, or
/// empty when the block is empty (so an absent check adds no blank line).
fn block1(s: &str) -> String {
    if s.is_empty() {
        String::new()
    } else {
        format!("{}\n", plan::nest("  ", s, 1))
    }
}
