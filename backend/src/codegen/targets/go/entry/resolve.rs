//! The Go leaf spellings for the shared resolution plan.
//!
//! The control flow (source ordering, absence tracking, config composition)
//! lives once in `codegen::entries::plan`; this file only says how each leaf
//! statement reads in Go and keeps the genuinely Go-specific constructs (the
//! `if v, ok := os.LookupEnv` env boundary, the typed `strconv`/`json.Decoder`
//! parses, the branded-string casts) that no other target shares.

use super::*;
use crate::codegen::entries::plan::{self, Cond, Emitter, Leaf, Stmt};

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

    fn ident(&self, name: &str) -> String {
        format!("s.{}", field_pascal(name, self.config))
    }

    /// The read expression of a sibling-field path (`creds.token` ->
    /// `s.Creds.Token`).
    fn path_expr(&self, path: &[String]) -> String {
        let mut out = "s".to_string();
        for seg in path {
            out.push('.');
            out.push_str(&field_pascal(seg, self.config));
        }
        out
    }

    fn path_type(&self, path: &[String]) -> Tref {
        self.entry.path_type(path, self.module)
    }

    fn guaranteed(&self, name: &str) -> bool {
        self.entry.field_guaranteed(name)
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

    fn env_lookup(&mut self, name: &EnvName) -> String {
        self.import("os", "os");
        match name {
            EnvName::Name(n) => format!("os.LookupEnv({n:?})"),
            EnvName::Field(fr) => {
                let expr = self.path_expr(&fr.field);
                let t = self.path_type(&fr.field);
                format!("os.LookupEnv({})", self.as_string_expr(&expr, &t))
            }
        }
    }

    /// [`as_string`] plus the fmt import its non-string spelling needs.
    fn as_string_expr(&mut self, expr: &str, t: &Tref) -> String {
        if as_string_needs_fmt(t) {
            self.import("fmt", "fmt");
        }
        as_string(expr, t)
    }

    fn env_label(&mut self, name: &EnvName) -> String {
        match name {
            EnvName::Name(n) => format!("{n:?}"),
            EnvName::Field(fr) => {
                let t = self.path_type(&fr.field);
                self.as_string_expr(&self.path_expr(&fr.field), &t)
            }
        }
    }

    fn env_miss_reason(&mut self, name: &EnvName) -> String {
        match name {
            EnvName::Name(n) => format!("{:?}", format!("env {n}: empty")),
            EnvName::Field(fr) => {
                let t = self.path_type(&fr.field);
                let expr = self.path_expr(&fr.field);
                format!(
                    "\"env \" + {} + \": empty\"",
                    self.as_string_expr(&expr, &t)
                )
            }
        }
    }

    /// The statements parsing a raw env string `v` into the destination, by the
    /// field's declared type; a parse failure fails construction naming the
    /// variable and the type. Relative to column zero (nested by the caller).
    fn env_parse(&mut self, field: &EntryField, dest: &str, label: &str) -> String {
        let t = &field.target;
        match t {
            Tref::Prim(Prim::Bool) => {
                self.import("fmt", "fmt");
                format!(
                    "switch v {{\ncase \"true\", \"1\":\n\t{dest} = true\ncase \"false\", \"0\":\n\t{dest} = false\ndefault:\n\treturn nil, fmt.Errorf(\"%s: invalid bool %q (want true/false/1/0)\", {label}, v)\n}}"
                )
            }
            Tref::Prim(p @ (Prim::I8 | Prim::I16 | Prim::I32 | Prim::I64)) => {
                self.import("strconv", "strconv");
                self.import("fmt", "fmt");
                format!(
                    "n, err := strconv.ParseInt(v, 10, {bits})\nif err != nil {{\n\treturn nil, fmt.Errorf(\"%s: invalid {prim} %q\", {label}, v)\n}}\n{dest} = {cast}(n)",
                    bits = int_bits(p),
                    prim = prim_name(p),
                    cast = prim_spelling(p).go,
                )
            }
            Tref::Prim(p @ (Prim::U8 | Prim::U16 | Prim::U32 | Prim::U64)) => {
                self.import("strconv", "strconv");
                self.import("fmt", "fmt");
                format!(
                    "n, err := strconv.ParseUint(v, 10, {bits})\nif err != nil {{\n\treturn nil, fmt.Errorf(\"%s: invalid {prim} %q\", {label}, v)\n}}\n{dest} = {cast}(n)",
                    bits = int_bits(p),
                    prim = prim_name(p),
                    cast = prim_spelling(p).go,
                )
            }
            Tref::Prim(Prim::Float) => {
                self.import("strconv", "strconv");
                self.import("strings", "strings");
                self.import("fmt", "fmt");
                // Decimal notation only: ParseFloat alone also accepts Inf,
                // NaN, hex floats, and digit separators, which the TypeScript
                // boundary rejects; the same env value must construct in both.
                format!(
                    "if strings.ContainsFunc(v, func(r rune) bool {{ return !strings.ContainsRune(\"0123456789+-.eE\", r) }}) {{\n\treturn nil, fmt.Errorf(\"%s: invalid float %q\", {label}, v)\n}}\nn, err := strconv.ParseFloat(v, 64)\nif err != nil {{\n\treturn nil, fmt.Errorf(\"%s: invalid float %q\", {label}, v)\n}}\n{dest} = n"
                )
            }
            Tref::Prim(Prim::Bytes) => {
                self.import("base64", "encoding/base64");
                self.import("fmt", "fmt");
                // The env boundary carries bytes the same way the wire does:
                // base64 text.
                format!(
                    "b, err := base64.StdEncoding.DecodeString(v)\nif err != nil {{\n\treturn nil, fmt.Errorf(\"%s: invalid base64 %q\", {label}, v)\n}}\n{dest} = b"
                )
            }
            Tref::Prim(Prim::Duration) => {
                self.import("time", "time");
                self.import("fmt", "fmt");
                format!(
                    "if _, err := time.ParseDuration(v); err != nil {{\n\treturn nil, fmt.Errorf(\"%s: invalid duration %q\", {label}, v)\n}}\n{dest} = Duration(v)"
                )
            }
            _ => format!("{dest} = {}", cast_string(t, "v")),
        }
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

    /// `field: T = match .subject { ... }` lowered to the native switch,
    /// relative to column zero.
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
                Some(p) => {
                    arms.push_str(&format!(
                        "case {}:\n{}\n",
                        pattern_literal(p),
                        nest(&stmts, 1)
                    ));
                }
                None => {
                    saw_wildcard = true;
                    arms.push_str(&format!("default:\n{}\n", nest(&stmts, 1)));
                }
            }
        }
        if !saw_wildcard {
            // Every enum is open, so an undeclared value can still arrive at
            // run time even with total declared coverage. Failing construction
            // beats freezing a silent zero value into the resolved settings.
            let miss = if guaranteed {
                self.import("fmt", "fmt");
                format!(
                    "return nil, fmt.Errorf(\"{field}: match on {subject}: unmatched value %v\", {subject_expr})",
                    field = field.name,
                    subject = subject_head,
                )
            } else {
                format!("{why} = \"match: unmatched value\"")
            };
            arms.push_str(&format!("default:\n{}\n", nest(&miss, 1)));
        }
        let switch = format!("switch {subject_expr} {{\n{arms}}}");
        let guarded = if self.guaranteed(&subject_head) {
            switch
        } else {
            format!(
                "if {subj_why} != \"\" {{\n\t{why} = \"{subject_head} <- \" + {subj_why}\n}} else {{\n{sw}\n}}",
                subj_why = why_var(&subject_head),
                sw = nest(&switch, 1),
            )
        };
        if guaranteed {
            guarded
        } else {
            format!("{why} := \"\"\n{guarded}")
        }
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
            ArmValue::Lit(v) => format!("{dest} = {}", literal(&field.target, v)),
            ArmValue::Field(path) => {
                let head = path.first().cloned().unwrap_or_default();
                let expr = self.path_expr(path);
                if self.guaranteed(&head) {
                    format!("{dest} = {expr}")
                } else {
                    format!(
                        "if {head_why} != \"\" {{\n\t{why} = \"{head} <- \" + {head_why}\n}} else {{\n\t{dest} = {expr}\n}}",
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
                        "{why} = \"no source\"\n{}",
                        self.chain_sequential(&stub, dest, why)
                    )
                }
            }
        }
    }

    /// A non-guaranteed chain body (used inside a match arm), relative to column
    /// zero: the first source runs unconditionally, later ones guard on the
    /// why-var.
    fn chain_sequential(&mut self, field: &EntryField, dest: &str, why: &str) -> String {
        let mut out = String::new();
        let mut first = true;
        for source in &field.sources {
            let step = match source {
                Source::With => self.with_step_body(field, dest, why),
                Source::Env(name) => self.env_step_body(field, name, dest, why),
                Source::Default(v) => {
                    format!("{dest} = {}\n{why} = \"\"", literal(&field.target, v))
                }
                Source::Arg => continue,
            };
            if first {
                out.push_str(&step);
                out.push('\n');
                first = false;
            } else {
                out.push_str(&format!("if {why} != \"\" {{\n{}\n}}\n", nest(&step, 1)));
            }
        }
        out
    }

    /// The `@with` presence step of a non-guaranteed chain, relative to column
    /// zero.
    fn with_step_body(&self, field: &EntryField, dest: &str, why: &str) -> String {
        format!(
            "if w.{carrier} != nil {{\n\t{dest} = *w.{carrier}\n\t{why} = \"\"\n}} else {{\n\t{why} = \"not configured\"\n}}",
            carrier = camel(&field.name),
        )
    }

    /// `@format` template plus the `@str::*` pipeline, relative to column zero.
    fn format_body(&mut self, field: &EntryField, dest: &str) -> String {
        let Some(parts) = field.format.clone() else {
            return String::new();
        };
        let (concat, absent_deps) = self.format_pieces(&parts);
        if !field.transforms.is_empty() {
            self.import("strings", "strings");
        }
        let expr = apply_transforms(concat.join(" + "), &field.transforms, self.helpers);
        let assign = format!("{dest} = {}", cast_string(&field.target, &expr));
        if absent_deps.is_empty() {
            return assign;
        }
        let why = why_var(&field.name);
        let mut chain = String::new();
        for (i, dep) in absent_deps.iter().enumerate() {
            chain.push_str(&format!(
                "{}if {dep_why} != \"\" {{\n\t{why} = \"{dep} <- \" + {dep_why}\n}}",
                if i == 0 { "" } else { " else " },
                dep_why = why_var(dep),
            ));
        }
        chain.push_str(&format!(" else {{\n\t{assign}\n}}"));
        format!("{why} := \"\"\n{chain}")
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

    /// The `@arg`/`@with` opening shared by the structured and whole-JSON
    /// decodes: an `@arg` value passes typed (returns `None`, having written the
    /// assignment), otherwise the why-var opens and an optional `@with` layer
    /// wins, and the env source's `(dest, why, lookup, label, miss)` are
    /// returned for the decode body.
    fn decode_opening(
        &mut self,
        field: &EntryField,
        out: &mut String,
    ) -> Option<(String, String, String, String, String)> {
        let dest = self.ident(&field.name);
        if field.sources.iter().any(|s| matches!(s, Source::Arg)) {
            out.push_str(&format!("{dest} = {}", camel(&field.name)));
            return None;
        }
        let why = why_var(&field.name);
        out.push_str(&format!("{why} := \"no source\"\n"));
        if field.sources.iter().any(|s| matches!(s, Source::With)) {
            out.push_str(&format!(
                "if w.{carrier} != nil {{\n\t{dest} = *w.{carrier}\n\t{why} = \"\"\n}} else {{\n\t{why} = \"not configured\"\n}}\n",
                carrier = camel(&field.name),
            ));
        }
        let Some(Source::Env(name)) = field.sources.iter().find(|s| matches!(s, Source::Env(_)))
        else {
            return None;
        };
        self.import("os", "os");
        self.import("json", "encoding/json");
        self.import("fmt", "fmt");
        let lookup = self.env_lookup(name);
        let label = self.env_label(name);
        let miss = self.env_miss_reason(name);
        Some((dest, why, lookup, label, miss))
    }

    /// A structured source: an `@arg`/`@with` value passes typed, a JSON env
    /// value decodes strictly into the wire struct. Relative to column zero.
    fn structured_body(&mut self, field: &EntryField, shape: &Shape) -> String {
        let mut out = String::new();
        let Some((dest, why, lookup, label, miss)) = self.decode_opening(field, &mut out) else {
            return out;
        };
        self.import("bytes", "bytes");
        let ty = type_ident_from_id(&shape.id);
        let mut required_checks = String::new();
        if let ShapeKind::Structure { members, .. } = &shape.kind {
            for m in members.iter().filter(|m| m.required) {
                // An explicit null is as absent as a missing key: the typed
                // decode below would zero it silently, and the TypeScript decode
                // rejects it, so both targets treat it as missing.
                required_checks.push_str(&format!(
                    "\tif rv, ok := probe[{name:?}]; !ok || string(rv) == \"null\" {{\n\t\treturn nil, fmt.Errorf(\"%s: missing field {name}\", {label})\n\t}}\n",
                    name = m.name,
                ));
            }
        }
        let validate = if validation::shape_has_checks(shape) {
            let en = error_names();
            format!(
                "\tif vs := Validate{ty}(decoded); len(vs) > 0 {{\n\t\treturn nil, &{validation}{{Violations: vs}}\n\t}}\n",
                validation = en.validation,
            )
        } else {
            String::new()
        };
        let block = format!(
            "if raw, ok := {lookup}; ok && raw != \"\" {{\n\
             \tvar probe map[string]json.RawMessage\n\
             \tif err := json.Unmarshal([]byte(raw), &probe); err != nil {{\n\t\treturn nil, fmt.Errorf(\"%s: %v\", {label}, err)\n\t}}\n\
             {required_checks}\
             \tdec := json.NewDecoder(bytes.NewReader([]byte(raw)))\n\
             \tdec.DisallowUnknownFields()\n\
             \tvar decoded {ty}\n\
             \tif err := dec.Decode(&decoded); err != nil {{\n\t\treturn nil, fmt.Errorf(\"%s: %v\", {label}, err)\n\t}}\n\
             {validate}\
             \t{dest} = decoded\n\
             \t{why} = \"\"\n\
             }} else {{\n\t{why} = {miss}\n}}"
        );
        // An explicit @with value wins: the decode runs only while unset.
        out.push_str(&format!("if {why} != \"\" {{\n{}\n}}", nest(&block, 1)));
        out
    }

    /// A map/list field: an `@arg`/`@with` value passes typed, an env value
    /// decodes as JSON whole. Relative to column zero.
    fn json_body(&mut self, field: &EntryField) -> String {
        let mut out = String::new();
        let Some((dest, why, lookup, label, miss)) = self.decode_opening(field, &mut out) else {
            return out;
        };
        let block = format!(
            "if raw, ok := {lookup}; ok && raw != \"\" {{\n\
             \tif err := json.Unmarshal([]byte(raw), &{dest}); err != nil {{\n\t\treturn nil, fmt.Errorf(\"%s: %v\", {label}, err)\n\t}}\n\
             \t{why} = \"\"\n\
             }} else {{\n\t{why} = {miss}\n}}"
        );
        out.push_str(&format!("if {why} != \"\" {{\n{}\n}}", nest(&block, 1)));
        out
    }

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
                    "if v, ok := {lookup}; ok && v != \"\" {{\n{}\n}}",
                    nest(&parse, 1),
                ));
            }
        }
        out
    }

    /// A member match, lowered without a why-var: an absent subject or an
    /// unmatched value leaves the member's zero value. Relative to column zero.
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
                Some(p) => {
                    arms.push_str(&format!(
                        "case {}:\n{}\n",
                        pattern_literal(p),
                        nest(&stmts, 1)
                    ));
                }
                None => arms.push_str(&format!("default:\n{}\n", nest(&stmts, 1))),
            }
        }
        let switch = format!("switch {subject_expr} {{\n{arms}}}");
        if self.guaranteed(&subject_head) {
            switch
        } else {
            format!(
                "if {subj_why} == \"\" {{\n{}\n}}",
                nest(&switch, 1),
                subj_why = why_var(&subject_head),
            )
        }
    }

    fn member_arm_body(&mut self, member: &EntryField, value: &ArmValue, dest: &str) -> String {
        match value {
            ArmValue::Lit(v) => format!("{dest} = {}", literal(&member.target, v)),
            ArmValue::Field(path) => {
                let head = path.first().cloned().unwrap_or_default();
                let expr = self.path_expr(path);
                if self.guaranteed(&head) {
                    format!("{dest} = {expr}")
                } else {
                    format!(
                        "if {head_why} == \"\" {{\n\t{dest} = {expr}\n}}",
                        head_why = why_var(&head),
                    )
                }
            }
            ArmValue::Sources(sources) => {
                self.member_chain_body(&plan::arm_sources(member, sources), dest)
            }
        }
    }

    /// The `@str::*` pipeline over an already-resolved destination, relative to
    /// column zero, or `None` when the field declares no transforms.
    fn transforms_body(&mut self, field: &EntryField, dest: &str) -> Option<String> {
        if field.transforms.is_empty() {
            return None;
        }
        self.import("strings", "strings");
        let expr = self.as_string_expr(dest, &field.target);
        let expr = apply_transforms(expr, &field.transforms, self.helpers);
        Some(format!("{dest} = {}", cast_string(&field.target, &expr)))
    }
}

impl Emitter for Resolver<'_, '_> {
    fn indent_unit(&self) -> &'static str {
        "\t"
    }

    fn if_header(&self, cond: &Cond) -> String {
        format!("if {}", cond.0)
    }

    fn dest(&self, field_name: &str) -> String {
        self.ident(field_name)
    }

    fn member_dest(&self, member_name: &str) -> String {
        format!("composed.{}", field_pascal(member_name, self.config))
    }

    fn assign_arg(&mut self, field: &EntryField, dest: &str) -> Leaf {
        Leaf(format!("{dest} = {}", camel(&field.name)))
    }

    fn assign_default(
        &mut self,
        field: &EntryField,
        value: &serde_json::Value,
        dest: &str,
    ) -> Leaf {
        Leaf(format!("{dest} = {}", literal(&field.target, value)))
    }

    fn why_open(&self, field_name: &str, initial: &str) -> Leaf {
        Leaf(format!("{} := {initial:?}", why_var(field_name)))
    }

    fn why_set(&self, why_field: &str, reason: &str) -> Leaf {
        Leaf(format!("{} = {reason:?}", why_var(why_field)))
    }

    fn cond_why_absent(&self, field_name: &str) -> Cond {
        Cond(format!("{} != \"\"", why_var(field_name)))
    }

    fn cond_why_resolved(&self, field_name: &str) -> Cond {
        Cond(format!("{} == \"\"", why_var(field_name)))
    }

    fn with_step(&mut self, field: &EntryField, dest: &str, why_field: &str) -> Leaf {
        Leaf(self.with_step_body(field, dest, &why_var(why_field)))
    }

    fn env_step(
        &mut self,
        field: &EntryField,
        source: &Source,
        dest: &str,
        why_field: &str,
    ) -> Leaf {
        let Source::Env(name) = source else {
            return Leaf(String::new());
        };
        Leaf(self.env_step_body(field, name, dest, &why_var(why_field)))
    }

    fn chain_guaranteed_leaf(&mut self, field: &EntryField, dest: &str) -> Leaf {
        Leaf(self.chain_guaranteed(field, dest))
    }

    fn select_leaf(&mut self, field: &EntryField, dest: &str) -> Leaf {
        Leaf(self.select_body(field, dest))
    }

    fn format_leaf(&mut self, field: &EntryField, dest: &str) -> Leaf {
        Leaf(self.format_body(field, dest))
    }

    fn transforms_leaf(&mut self, field: &EntryField, dest: &str) -> Option<Leaf> {
        self.transforms_body(field, dest).map(Leaf)
    }

    fn structured_leaf(&mut self, field: &EntryField, shape: &Shape) -> Stmt {
        Stmt::Leaf(Leaf(self.structured_body(field, shape)))
    }

    fn json_leaf(&mut self, field: &EntryField) -> Stmt {
        Stmt::Leaf(Leaf(self.json_body(field)))
    }

    fn config_open(&mut self, _field: &EntryField, shape: &Shape) -> Leaf {
        Leaf(format!("var composed {}", type_ident_from_id(&shape.id)))
    }

    fn config_close(&self, dest: &str) -> Leaf {
        Leaf(format!("{dest} = composed"))
    }

    fn bind_expr(&self, source: &[String]) -> String {
        self.path_expr(source)
    }

    fn member_bind_assign(&self, member_dest: &str, expr: &str) -> Leaf {
        Leaf(format!("{member_dest} = {expr}"))
    }

    fn member_select_leaf(&mut self, member: &EntryField, dest: &str) -> Leaf {
        Leaf(self.member_select_body(member, dest))
    }

    fn member_format_leaf(&mut self, member: &EntryField, dest: &str) -> Leaf {
        Leaf(self.format_body(member, dest))
    }

    fn member_chain(&mut self, member: &EntryField, sources: &[Source], dest: &str) -> Stmt {
        Stmt::Leaf(Leaf(
            self.member_chain_body(&plan::arm_sources(member, sources), dest),
        ))
    }

    fn member_transforms_leaf(&mut self, member: &EntryField, dest: &str) -> Option<Leaf> {
        self.transforms_body(member, dest).map(Leaf)
    }
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
