//! The Go leaf spellings for the shared resolution plan.
//!
//! The control flow (source ordering, absence tracking, config composition)
//! lives once in `codegen::entries::plan`; this file only says how each leaf
//! statement reads in Go and keeps the genuinely Go-specific constructs (the
//! `if v, ok := os.LookupEnv` env boundary, the typed `strconv`/`json.Decoder`
//! parses, the branded-string casts) that no other target shares.

use super::*;
use crate::codegen::entries::plan::{self, Cond, Emitter, Leaf};

/// The Go resolution emitter: holds the entry model and collects imports as the
/// plan is built. `body` receives the rendered plan for each field.
pub(super) struct Resolver<'a, 'b> {
    pub(super) entry: &'a EntryModel<'a>,
    pub(super) module: &'a Module,
    pub(super) config: &'a CasingConfig,
    pub(super) helpers: &'b mut Helpers,
    pub(super) refs: &'b mut Vec<Symbol>,
    pub(super) body: &'b mut String,
}

impl Resolver<'_, '_> {
    fn import(&mut self, name: &str, module: &str) {
        self.refs.push(import(name, module));
    }

    fn guaranteed(&self, name: &str) -> bool {
        self.entry.field_guaranteed(name)
    }

    /// The prereq guard when the env variable's own name comes from a sibling
    /// field that may itself be absent. Returns the opened guard text (the env
    /// step closes it), or empty when the name is guaranteed.
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
            "if {head_why} != \"\" {{\n\t{why} = \"{head} <- \" + {head_why}\n}} else ",
            head_why = why_var(head),
        )
    }

    /// [`as_string`] plus the fmt import its non-string spelling needs.
    fn as_string_expr(&mut self, expr: &str, t: &Tref) -> String {
        if as_string_needs_fmt(t) {
            self.import("fmt", "fmt");
        }
        as_string(expr, t)
    }

    /// The statements parsing a raw env string `v` into the destination, by the
    /// field's declared type; a parse failure fails construction naming the
    /// variable and the type. Relative to column zero (nested by the caller).
    fn env_parse(&mut self, field: &EntryField, dest: &str, label: &str) -> String {
        let t = &field.target;
        match t {
            Tref::Prim(Prim::Bool) => {
                self.import("fmt", "fmt");
                let fail = config_errorf(&format!(
                    "\"%s: invalid bool %q (want true/false/1/0)\", {label}, v"
                ));
                format!(
                    "switch v {{\ncase \"true\", \"1\":\n\t{dest} = true\ncase \"false\", \"0\":\n\t{dest} = false\ndefault:\n\t{fail}\n}}"
                )
            }
            Tref::Prim(p @ (Prim::I8 | Prim::I16 | Prim::I32 | Prim::I64)) => {
                self.import("strconv", "strconv");
                self.import("fmt", "fmt");
                let fail = config_errorf(&format!(
                    "\"%s: invalid {prim} %q\", {label}, v",
                    prim = prim_name(p)
                ));
                format!(
                    "n, err := strconv.ParseInt(v, 10, {bits})\nif err != nil {{\n\t{fail}\n}}\n{dest} = {cast}(n)",
                    bits = int_bits(p),
                    cast = prim_spelling(p).go,
                )
            }
            Tref::Prim(p @ (Prim::U8 | Prim::U16 | Prim::U32 | Prim::U64)) => {
                self.import("strconv", "strconv");
                self.import("fmt", "fmt");
                let fail = config_errorf(&format!(
                    "\"%s: invalid {prim} %q\", {label}, v",
                    prim = prim_name(p)
                ));
                format!(
                    "n, err := strconv.ParseUint(v, 10, {bits})\nif err != nil {{\n\t{fail}\n}}\n{dest} = {cast}(n)",
                    bits = int_bits(p),
                    cast = prim_spelling(p).go,
                )
            }
            Tref::Prim(Prim::Float) => {
                self.import("strconv", "strconv");
                self.import("strings", "strings");
                self.import("fmt", "fmt");
                let fail = config_errorf(&format!("\"%s: invalid float %q\", {label}, v"));
                // Decimal notation only: ParseFloat alone also accepts Inf,
                // NaN, hex floats, and digit separators, which the TypeScript
                // boundary rejects; the same env value must construct in both.
                format!(
                    "if strings.ContainsFunc(v, func(r rune) bool {{ return !strings.ContainsRune(\"0123456789+-.eE\", r) }}) {{\n\t{fail}\n}}\nn, err := strconv.ParseFloat(v, 64)\nif err != nil {{\n\t{fail}\n}}\n{dest} = n"
                )
            }
            Tref::Prim(Prim::Bytes) => {
                self.import("base64", "encoding/base64");
                self.import("fmt", "fmt");
                let fail = config_errorf(&format!("\"%s: invalid base64 %q\", {label}, v"));
                // The env boundary carries bytes the same way the wire does:
                // base64 text.
                format!(
                    "b, err := base64.StdEncoding.DecodeString(v)\nif err != nil {{\n\t{fail}\n}}\n{dest} = b"
                )
            }
            Tref::Prim(Prim::Duration) => {
                self.import("time", "time");
                self.import("fmt", "fmt");
                let fail = config_errorf(&format!("\"%s: invalid duration %q\", {label}, v"));
                format!(
                    "if _, err := time.ParseDuration(v); err != nil {{\n\t{fail}\n}}\n{dest} = Duration(v)"
                )
            }
            _ => format!("{dest} = {}", cast_string(t, "v")),
        }
    }

    /// The template split: each part as a Go string expression, plus the
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
                    concat.push(self.as_string_expr(&expr, &t));
                }
                // An op-input placeholder cannot appear in a field template;
                // the frontend rejects it. Render empty defensively.
                TemplatePart::Input(_) => concat.push("\"\"".to_string()),
            }
        }
        (concat, absent_deps)
    }

    /// The `@arg`/why-var opening shared by the structured and whole-JSON
    /// decodes: an `@arg` value passes typed (returns `None`, having written the
    /// assignment), otherwise the why-var opens and `(dest, why)` are returned so
    /// the caller can lay out the `@with`/`@env`/`@default` fallback cascade.
    fn decode_opening(&mut self, field: &EntryField, out: &mut String) -> Option<(String, String)> {
        let dest = self.ident(&field.name);
        if field.sources.iter().any(|s| matches!(s, Source::Arg)) {
            out.push_str(&format!("{dest} = {}", self.arg_ident(field)));
            return None;
        }
        let why = why_var(&field.name);
        out.push_str(&format!("{why} := \"no source\"\n"));
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
                    self.import("os", "os");
                    self.import("json", "encoding/json");
                    self.import("fmt", "fmt");
                    let lookup = self.env_lookup(name);
                    let label = self.env_label(name);
                    let miss = self.env_miss_reason(name);
                    let pre = self.env_name_prereq(name, why);
                    steps.push(env_block(self, &lookup, &label, &miss, &pre));
                }
                Source::Default(v) => steps.push(format!(
                    "{dest} = {}\n{why} = \"\"",
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
                    format!("if {why} != \"\" {{\n{}\n}}", nest(step, 1))
                }
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}

impl Emitter for Resolver<'_, '_> {
    fn indent_unit(&self) -> &'static str {
        "\t"
    }

    fn lang(&self) -> &'static str {
        LANG
    }

    fn term(&self) -> &'static str {
        ""
    }

    fn eq(&self) -> &'static str {
        "=="
    }

    fn neq(&self) -> &'static str {
        "!="
    }

    fn if_header(&self, cond: &Cond) -> String {
        format!("if {}", cond.0)
    }

    fn ident(&self, name: &str) -> String {
        format!(
            "s.{}",
            field_pascal_ren(
                name,
                self.entry.field_rename(name, LANG).as_deref(),
                self.config
            )
        )
    }

    fn path_expr(&self, path: &[String]) -> String {
        let mut out = "s".to_string();
        for (i, seg) in path.iter().enumerate() {
            out.push('.');
            // Only the head is an entry field (it honors @rename); the tail
            // reaches into config/struct members, spelled plainly.
            if i == 0 {
                out.push_str(&field_pascal_ren(
                    seg,
                    self.entry.field_rename(seg, LANG).as_deref(),
                    self.config,
                ));
            } else {
                out.push_str(&field_pascal(seg, self.config));
            }
        }
        out
    }

    fn path_type(&self, path: &[String]) -> Tref {
        self.entry.path_type(path, self.module)
    }

    fn member_dest(&self, member_name: &str) -> String {
        format!("composed.{}", field_pascal(member_name, self.config))
    }

    fn literal_of(&self, target: &Tref, value: &serde_json::Value) -> String {
        literal(target, value)
    }

    fn to_string_expr(&mut self, expr: &str, t: &Tref) -> String {
        self.as_string_expr(expr, t)
    }

    fn env_read_call(&mut self, name_expr: &str) -> String {
        self.import("os", "os");
        format!("os.LookupEnv({name_expr})")
    }

    fn why_open(&self, field_name: &str, initial: &str) -> Leaf {
        Leaf(format!("{} := {initial:?}", why_var(field_name)))
    }

    fn config_open(&mut self, _field: &EntryField, shape: &Shape) -> Leaf {
        Leaf(format!("var composed {}", config_type_ident(&shape.id)))
    }

    /// The `@with` presence step of a non-guaranteed chain, relative to column
    /// zero.
    fn with_step_body(&self, field: &EntryField, dest: &str, why: &str) -> String {
        format!(
            "if w.{carrier} != nil {{\n\t{dest} = *w.{carrier}\n\t{why} = \"\"\n}} else {{\n\t{why} = \"not configured\"\n}}",
            carrier = camel(&field.name),
        )
    }

    /// The env presence step: `if v, ok := lookup; ok && v != "" { parse } else
    /// { miss }`, prefixed by the name prereq when the variable name is itself a
    /// non-guaranteed sibling. Relative to column zero.
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
            "{pre}if v, ok := {lookup}; ok && v != \"\" {{\n{parse}\n\t{why} = \"\"\n}} else {{\n\t{why} = {miss}\n}}",
            parse = nest(&parse, 1),
        )
    }

    /// A guaranteed chain as an if/else cascade ending in `@default`, relative
    /// to column zero.
    fn chain_guaranteed(&mut self, field: &EntryField, dest: &str) -> String {
        let mut out = String::new();
        let mut first = true;
        for source in &field.sources {
            match source {
                Source::With => {
                    out.push_str(&format!(
                        "{}if w.{carrier} != nil {{\n\t{dest} = *w.{carrier}\n}}",
                        if first { "" } else { " else " },
                        carrier = camel(&field.name),
                    ));
                    first = false;
                }
                Source::Env(name) => {
                    let lookup = self.env_lookup(name);
                    let label = self.env_label(name);
                    let parse = self.env_parse(field, dest, &label);
                    out.push_str(&format!(
                        "{}if v, ok := {lookup}; ok && v != \"\" {{\n{parse}\n}}",
                        if first { "" } else { " else " },
                        parse = nest(&parse, 1),
                    ));
                    first = false;
                }
                Source::Default(v) => {
                    let lit = literal(&field.target, v);
                    if first {
                        out.push_str(&format!("{dest} = {lit}"));
                    } else {
                        out.push_str(&format!(" else {{\n\t{dest} = {lit}\n}}"));
                    }
                    return out;
                }
                Source::Arg => {}
            }
        }
        out
    }

    fn switch_header(&self, subject: &str) -> String {
        format!("switch {subject}")
    }

    fn case_open(&self, pattern: &str) -> String {
        format!("case {pattern}:")
    }

    fn default_open(&self) -> String {
        "default:".to_string()
    }

    fn case_tail(&self) -> Option<&'static str> {
        None
    }

    fn case_close(&self) -> Option<&'static str> {
        None
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
            self.import("fmt", "fmt");
            Leaf(config_errorf(&format!(
                "\"{field}: match on {subject}: unmatched value %v\", {subject_expr}",
                field = field.name,
                subject = subject_head,
            )))
        } else {
            Leaf(format!(
                "{} = \"match: unmatched value\"",
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
        if !field.transforms.is_empty() {
            self.import("strings", "strings");
        }
        let expr = apply_transforms(
            concat.join(" + "),
            &field.transforms,
            self.helpers,
            self.refs,
        );
        format!("{dest} = {}", cast_string(&field.target, &expr))
    }

    /// The `@str::*` pipeline over an already-resolved destination, relative to
    /// column zero, or `None` when the field declares no transforms.
    fn transforms_body(&mut self, field: &EntryField, dest: &str) -> Option<String> {
        if field.transforms.is_empty() {
            return None;
        }
        self.import("strings", "strings");
        let expr = self.as_string_expr(dest, &field.target);
        let expr = apply_transforms(expr, &field.transforms, self.helpers, self.refs);
        Some(format!("{dest} = {}", cast_string(&field.target, &expr)))
    }

    /// A structured source: an `@arg`/`@with` value passes typed, a JSON env
    /// value decodes strictly into the wire struct. Relative to column zero.
    fn structured_body(&mut self, field: &EntryField, shape: &Shape) -> String {
        let mut out = String::new();
        let Some((dest, why)) = self.decode_opening(field, &mut out) else {
            return out;
        };
        self.import("bytes", "bytes");
        let ty = type_ident_from_id(&shape.id);
        let mut required_checks = String::new();
        if let ShapeKind::Structure { members, .. } = &shape.kind {
            for m in members.iter().filter(|m| m.required) {
                // An explicit null is as absent as a missing key: the typed
                // decode below would zero it silently, and the TypeScript decode
                // rejects it, so both targets treat it as missing. The probe reads
                // the wire key (the @wire override the codec serializes under), not
                // the in-code member name.
                let name = wire_key(m);
                let fail = config_errorf(&format!("\"%s: missing field {name}\", {{LABEL}}"));
                required_checks.push_str(&format!(
                    "\tif rv, ok := probe[{name:?}]; !ok || string(rv) == \"null\" {{\n\t\t{fail}\n\t}}\n",
                ));
            }
        }
        let validate = if validation::shape_has_checks(shape) {
            format!(
                "\tif invalid := Validate{ty}(decoded); invalid != nil {{\n\t\treturn nil, invalid\n\t}}\n"
            )
        } else {
            String::new()
        };
        // The decode body varies only by the env label; the cascade fills each
        // source's own `(lookup, label, miss)` in around this shared shape.
        let (dc, wc) = (dest.clone(), why.clone());
        let body = self.decode_cascade(field, &dest, &why, move |_, lookup, label, miss, pre| {
            let required = required_checks.replace("{LABEL}", label);
            let fail = config_errorf(&format!("\"%s: %v\", {label}, err"));
            format!(
                "{pre}if raw, ok := {lookup}; ok && raw != \"\" {{\n\
                 \tvar probe map[string]json.RawMessage\n\
                 \tif err := json.Unmarshal([]byte(raw), &probe); err != nil {{\n\t\t{fail}\n\t}}\n\
                 {required}\
                 \tdec := json.NewDecoder(bytes.NewReader([]byte(raw)))\n\
                 \tdec.DisallowUnknownFields()\n\
                 \tvar decoded {ty}\n\
                 \tif err := dec.Decode(&decoded); err != nil {{\n\t\t{fail}\n\t}}\n\
                 {validate}\
                 \t{dc} = decoded\n\
                 \t{wc} = \"\"\n\
                 }} else {{\n\t{wc} = {miss}\n}}"
            )
        });
        out.push_str(&body);
        out
    }

    /// A map/list field: an `@arg`/`@with` value passes typed, an env value
    /// decodes as JSON whole. Relative to column zero.
    fn json_body(&mut self, field: &EntryField) -> String {
        let mut out = String::new();
        let Some((dest, why)) = self.decode_opening(field, &mut out) else {
            return out;
        };
        let (dc, wc) = (dest.clone(), why.clone());
        let body = self.decode_cascade(field, &dest, &why, move |_, lookup, label, miss, pre| {
            let fail = config_errorf(&format!("\"%s: %v\", {label}, err"));
            format!(
                "{pre}if raw, ok := {lookup}; ok && raw != \"\" {{\n\
                 \tif err := json.Unmarshal([]byte(raw), &{dc}); err != nil {{\n\t\t{fail}\n\t}}\n\
                 \t{wc} = \"\"\n\
                 }} else {{\n\t{wc} = {miss}\n}}"
            )
        });
        out.push_str(&body);
        out
    }

    fn require_member(&mut self, head: &str, member: &str, leaf: &Tref, name: &str) -> String {
        format!(
            "if s.{head_ident}.{member_ident} == {zero} {{\n\treturn nil, &{config}{{Message: \"{name}: no value\"}}\n}}",
            head_ident = field_pascal_ren(head, self.entry.field_rename(head, LANG).as_deref(), self.config),
            member_ident = field_pascal(member, self.config),
            zero = cast_string(leaf, "\"\""),
            config = error_names().config,
        )
    }

    fn require_member_deferred(&mut self, name: &str, why: &str) -> String {
        format!(
            "if {why} != \"\" {{\n\treturn nil, &{config}{{Message: \"{name} <- \" + {why}}}\n}}",
            why = why_var(why),
            config = error_names().config,
        )
    }

    fn require_string(&mut self, head: &str, target: &Tref) -> String {
        format!(
            "if s.{ident} == {zero} {{\n\twhy := {why}\n\tif why == \"\" {{\n\t\twhy = \"no value\"\n\t}}\n\treturn nil, &{config}{{Message: \"{name} <- \" + why}}\n}}",
            ident = field_pascal_ren(head, self.entry.field_rename(head, LANG).as_deref(), self.config),
            zero = cast_string(target, "\"\""),
            why = why_var(head),
            name = head,
            config = error_names().config,
        )
    }

    fn require_bytes(&mut self, head: &str) -> String {
        format!(
            "if len(s.{ident}) == 0 {{\n\twhy := {why}\n\tif why == \"\" {{\n\t\twhy = \"no value\"\n\t}}\n\treturn nil, &{config}{{Message: \"{name} <- \" + why}}\n}}",
            ident = field_pascal_ren(head, self.entry.field_rename(head, LANG).as_deref(), self.config),
            why = why_var(head),
            name = head,
            config = error_names().config,
        )
    }

    fn require_numeric(&mut self, head: &str, _target: &Tref) -> String {
        format!(
            "if {why} != \"\" && s.{ident} == 0 {{\n\treturn nil, &{config}{{Message: \"{name} <- \" + {why}}}\n}}",
            ident = field_pascal_ren(head, self.entry.field_rename(head, LANG).as_deref(), self.config),
            why = why_var(head),
            name = head,
            config = error_names().config,
        )
    }
}

/// A construction-time failure returning the SDK's dedicated ConfigError
/// category (message formatted from `sprintf_args`, the argument list a
/// `fmt.Errorf` would take). Every bad env value, malformed blob, absent
/// member, or unmatched select is a config problem, discriminable via
/// `errors.As` from a transport, validation, or declared error. Callers that
/// use this must import `fmt`.
pub(super) fn config_errorf(sprintf_args: &str) -> String {
    format!(
        "return nil, &{config}{{Message: fmt.Sprintf({sprintf_args})}}",
        config = error_names().config,
    )
}

/// Indent every non-empty line of `s` by `n` tabs, dropping any trailing
/// newline so callers control the separator.
fn nest(s: &str, n: usize) -> String {
    let pad = "\t".repeat(n);
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

fn int_bits(p: &Prim) -> u32 {
    match p {
        Prim::I8 | Prim::U8 => 8,
        Prim::I16 | Prim::U16 => 16,
        Prim::I32 | Prim::U32 => 32,
        _ => 64,
    }
}

fn prim_name(p: &Prim) -> &'static str {
    match p {
        Prim::I8 => "i8",
        Prim::I16 => "i16",
        Prim::I32 => "i32",
        Prim::I64 => "i64",
        Prim::U8 => "u8",
        Prim::U16 => "u16",
        Prim::U32 => "u32",
        Prim::U64 => "u64",
        _ => "value",
    }
}
