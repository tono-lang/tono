//! The Rust leaf spellings for the shared resolution plan.
//!
//! The control flow (source ordering, absence tracking, config composition)
//! lives once in `codegen::entries::plan`; this file only says how each leaf
//! statement reads in Rust. Two adaptations run throughout, both forced by
//! Rust having no implicit string coercion the way Go/TypeScript do:
//!
//! - every `<field>_why` reason variable is a real, owned `String` (never a
//!   bare `&'static str`), because it is reassigned with dynamically built
//!   messages (`format!("{head} <- {}", ..)`) as the resolution chain runs —
//!   several `Emitter` defaults that hard-code a bare string-literal
//!   assignment are overridden here to append `.to_string()`/wrap in
//!   `format!` accordingly;
//! - a `match` selection compares the Display-stringified subject against
//!   Display-stringified patterns (see `switch_header`) rather than native
//!   Rust match-pattern syntax, so one spelling covers a `String`, a numeric,
//!   or an open-enum subject without needing its type at the call site.

use super::*;
use crate::codegen::entries::plan::{Cond, Emitter, Leaf};
use crate::ir::EnvName;

/// The Rust resolution emitter: holds the entry model and flags the shared
/// on-demand helpers the leaves use. `body` receives the rendered plan for
/// each field; `refs` collects the `Symbol` for every SDK-root resolution
/// helper a leaf actually calls, so the import is collected and the
/// assembler's pruning ships only what this entry reaches. `arg_prefix` is
/// `"self."` when reading an `@arg` value off a builder (`build(self)`
/// consumes it) or empty when reading it off a bare function parameter
/// (`new`'s own arguments).
pub(super) struct Resolver<'a, 'b> {
    pub(super) entry: &'a EntryModel<'a>,
    pub(super) module: &'a Module,
    pub(super) config: &'a CasingConfig,
    pub(super) helpers: &'b mut Helpers,
    pub(super) arg_prefix: &'static str,
    pub(super) body: &'b mut String,
    pub(super) refs: &'b mut Vec<Symbol>,
}

/// An expression casting a Rust `String` into the field's target type. Only
/// valid for the string-shaped targets a `@format`/`@str::*` pipeline can
/// produce.
fn cast_string(t: &Tref, expr: &str) -> String {
    match t {
        Tref::Prim(Prim::String | Prim::Uuid) => expr.to_string(),
        Tref::Prim(Prim::Timestamp) => format!("Timestamp({expr})"),
        Tref::Prim(Prim::Date) => format!("LocalDate({expr})"),
        Tref::Prim(Prim::Duration) => format!("Duration({expr})"),
        // An open enum accepts any wire value; a dynamically derived string
        // (not a compile-time literal, so the known-variant lookup
        // `literal_enum` uses is unavailable here) always resolves through
        // the Unknown catch-all, which serializes identically to a named
        // variant carrying the same wire spelling.
        Tref::Ref { id, .. } => format!("{}::Unknown({expr})", type_ident_from_id(id)),
        _ => expr.to_string(),
    }
}

fn prim_rust_name(p: &Prim) -> &'static str {
    match p {
        Prim::I8 => "i8",
        Prim::I16 => "i16",
        Prim::I32 => "i32",
        Prim::I64 => "i64",
        Prim::U8 => "u8",
        Prim::U16 => "u16",
        Prim::U32 => "u32",
        Prim::U64 => "u64",
        _ => "i64",
    }
}

fn prim_label(p: &Prim) -> &'static str {
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

impl Resolver<'_, '_> {
    fn guaranteed(&self, name: &str) -> bool {
        self.entry.field_guaranteed(name)
    }

    fn arg_read(&self, field: &EntryField) -> String {
        format!(
            "{}{}",
            self.arg_prefix,
            arg_snake(&field.name, &field.traits, self.lang())
        )
    }

    /// The casing-transformed identifier for one canonical field name,
    /// honoring its `@rename(rust)` override — the leaf `ident` and
    /// `path_expr`'s own head segment both read a field through.
    fn field_ren(&self, name: &str) -> String {
        field_snake_ren(
            name,
            self.entry.field_rename(name, LANG).as_deref(),
            self.config,
        )
    }

    /// The prereq guard when the env variable's own name comes from a
    /// sibling field that may itself be absent; the env step chains onto its
    /// `else`.
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
            "if {head_why} != \"\" {{\n    {why} = format!(\"{head} <- {{}}\", {head_why});\n}} else ",
            head_why = why_var(head),
        )
    }

    /// The statements parsing a raw env string `v` into `dest`, by the
    /// field's declared type; a parse failure fails construction naming the
    /// variable (`label_expr`) and the type. Relative to column zero.
    fn env_parse(&mut self, field: &EntryField, dest: &str, label_expr: &str) -> String {
        let t = &field.target;
        match t {
            Tref::Prim(Prim::Bool) => {
                let fail = checks::config_error(&format!(
                    "format!(\"{{}}: invalid bool {{:?}} (want true/false/1/0)\", {label_expr}, v)"
                ));
                format!(
                    "match v.as_str() {{\n    \"true\" | \"1\" => {{ {dest} = true; }}\n    \"false\" | \"0\" => {{ {dest} = false; }}\n    _ => {{ {fail} }}\n}}"
                )
            }
            Tref::Prim(
                p @ (Prim::I8
                | Prim::I16
                | Prim::I32
                | Prim::I64
                | Prim::U8
                | Prim::U16
                | Prim::U32
                | Prim::U64),
            ) => {
                let ty = prim_rust_name(p);
                let fail = checks::config_error(&format!(
                    "format!(\"{{}}: invalid {prim} {{:?}}\", {label_expr}, v)",
                    prim = prim_label(p),
                ));
                format!("match v.parse::<{ty}>() {{\n    Ok(n) => {{ {dest} = n; }}\n    Err(_) => {{ {fail} }}\n}}")
            }
            Tref::Prim(Prim::Float) => {
                let fail = checks::config_error(&format!(
                    "format!(\"{{}}: invalid float {{:?}}\", {label_expr}, v)"
                ));
                // Decimal notation only: bare parse::<f64> also accepts
                // "inf"/"nan" spellings the Go/TypeScript boundary rejects.
                format!(
                    "if !v.chars().all(|c| c.is_ascii_digit() || \"+-.eE\".contains(c)) {{\n    {fail}\n}}\nmatch v.parse::<f64>() {{\n    Ok(n) if n.is_finite() => {{ {dest} = n; }}\n    _ => {{ {fail} }}\n}}"
                )
            }
            Tref::Prim(Prim::Bytes) => {
                self.refs.push(Symbol::imported(
                    "base64_bytes",
                    crate::codegen::group::Group::root("bytes").path(),
                    "base64_bytes",
                ));
                let fail = checks::config_error(&format!(
                    "format!(\"{{}}: invalid base64 {{:?}}\", {label_expr}, v)"
                ));
                format!("match {}::decode(&v) {{\n    Ok(b) => {{ {dest} = b; }}\n    Err(_) => {{ {fail} }}\n}}", shared_slot("base64_bytes"))
            }
            Tref::Prim(Prim::Duration) => {
                self.refs.push(shared_symbol("parse_duration_ms"));
                let fail = checks::config_error(&format!(
                    "format!(\"{{}}: invalid duration {{:?}}\", {label_expr}, v)"
                ));
                format!(
                    "if {}(&v).is_err() {{\n    {fail}\n}}\n{dest} = Duration(v.clone());",
                    shared_slot("parse_duration_ms")
                )
            }
            _ => format!("{dest} = {};", cast_string(t, "v")),
        }
    }

    /// Fold a field's declared sources into a presence cascade: `@with`
    /// shares its own step spelling ([`Emitter::with_step_body`]) across
    /// every target shape, and env is the only source whose step differs by
    /// target (a scalar/enum parse, a structured decode, a whole-JSON
    /// decode) — so the caller supplies just that one step, and everything
    /// around it (folding the steps into `if {why} != "" { ... }` guards) is
    /// written once. `env_step` takes `&mut Self` explicitly rather than
    /// capturing it, so it can still call the `&mut self` env helpers while
    /// this method holds its own `&mut self` borrow across the loop.
    fn source_cascade(
        &mut self,
        field: &EntryField,
        dest: &str,
        why: &str,
        mut env_step: impl FnMut(&mut Self, &EnvName) -> String,
    ) -> String {
        let mut steps: Vec<String> = Vec::new();
        for source in &field.sources {
            match source {
                Source::With => steps.push(self.with_step_body(field, dest, why)),
                Source::Env(name) => steps.push(env_step(self, name)),
                Source::Default(_) | Source::Arg => {}
            }
        }
        steps
            .iter()
            .enumerate()
            .map(|(i, step)| {
                if i == 0 {
                    step.clone()
                } else {
                    format!("if {why} != \"\" {{\n{}\n}}", indent(step, 1))
                }
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}

impl Emitter for Resolver<'_, '_> {
    fn indent_unit(&self) -> &'static str {
        "    "
    }

    fn lang(&self) -> &'static str {
        LANG
    }

    fn term(&self) -> &'static str {
        ";"
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
        format!("s.{}", self.field_ren(name))
    }

    fn path_expr(&self, path: &[String]) -> String {
        let mut out = "s".to_string();
        for (i, seg) in path.iter().enumerate() {
            out.push('.');
            if i == 0 {
                out.push_str(&self.field_ren(seg));
            } else {
                out.push_str(&field_snake(seg, self.config));
            }
        }
        out
    }

    fn path_type(&self, path: &[String]) -> Tref {
        self.entry.path_type(path, self.module)
    }

    fn literal_of(&self, target: &Tref, value: &serde_json::Value) -> String {
        literal(target, value, self.module)
    }

    /// Copy an already-resolved sibling field into `dest` (a match arm
    /// reading another field, `@bind`'s bound source). The default spelling
    /// (`dest = expr;`) would MOVE a non-`Copy` sibling (a `String`, a
    /// composed/structured value) out of `s`, and that sibling is routinely
    /// read again later (the value-freeze loop, another field's derivation) —
    /// every entry field type is `Clone`, so cloning here is what Go's value
    /// copy / TypeScript's reference read do implicitly.
    fn assign_expr(&self, dest: &str, expr: &str) -> Leaf {
        Leaf(format!("{dest} = ({expr}).clone();"))
    }

    fn member_dest(&self, member_name: &str) -> String {
        format!("composed.{}", field_snake(member_name, self.config))
    }

    fn arg_ident(&self, field: &EntryField) -> String {
        self.arg_read(field)
    }

    fn why_ident(&self, field_name: &str) -> String {
        why_var(field_name)
    }

    fn to_string_expr(&mut self, expr: &str, t: &Tref) -> String {
        match t {
            Tref::Prim(Prim::String | Prim::Uuid) => expr.to_string(),
            // Every other declared type (numeric, bool, the branded
            // well-known types, and an open enum) implements Display, either
            // natively or through the impl the generated serde file/types
            // file carries, so a plain `.to_string()` covers them all.
            _ => format!("{expr}.to_string()"),
        }
    }

    fn env_read_call(&mut self, name_expr: &str) -> String {
        self.refs.push(shared_symbol("read_env"));
        // `name_expr` is a `String`-typed expression when the variable name
        // is itself dynamic (`to_string_expr`'s output; a literal name is
        // `&'static str`, already reference-shaped); `read_env` takes `&str`,
        // and a leading `&` here satisfies both through deref coercion
        // (`&String -> &str`, `&&str -> &str`) without needing to know which
        // one it is.
        format!("{}(&{name_expr})", shared_slot("read_env"))
    }

    fn env_miss_reason(&mut self, name: &EnvName) -> String {
        match name {
            EnvName::Name(n) => format!("{:?}.to_string()", format!("env {n}: empty")),
            EnvName::Field(fr) => {
                let t = self.path_type_of(&fr.field);
                let read = self.path_read(&fr.field);
                let s = self.to_string_expr(&read, &t);
                format!("format!(\"env {{}}: empty\", {s})")
            }
        }
    }

    fn why_open(&self, field_name: &str, initial: &str) -> Leaf {
        Leaf(format!(
            "let mut {} = {initial:?}.to_string();",
            why_var(field_name)
        ))
    }

    fn why_set(&self, why_field: &str, reason: &str) -> Leaf {
        Leaf(format!(
            "{} = {reason:?}.to_string();",
            self.why_ident(why_field)
        ))
    }

    fn assign_reason(&self, why_field: &str, head: &str) -> Leaf {
        Leaf(format!(
            "{} = format!(\"{head} <- {{}}\", {});",
            self.why_ident(why_field),
            self.why_ident(head),
        ))
    }

    fn config_open(&mut self, field: &EntryField, _shape: &Shape) -> Leaf {
        Leaf(format!(
            "let mut composed = {};",
            zero_value(&field.target, self.module, self.config)
        ))
    }

    /// A `@bind` member assignment reads a sibling field, exactly the
    /// "copy, don't move" case [`Self::assign_expr`] documents.
    fn member_bind_assign(&self, member_dest: &str, expr: &str) -> Leaf {
        Leaf(format!("{member_dest} = ({expr}).clone();"))
    }

    /// The `@with` presence step of a non-guaranteed chain, relative to
    /// column zero.
    fn with_step_body(&self, field: &EntryField, dest: &str, why: &str) -> String {
        let acc = self.arg_read(field);
        format!(
            "if let Some(v) = {acc}.clone() {{\n    {dest} = v;\n    {why} = String::new();\n}} else {{\n    {why} = \"not configured\".to_string();\n}}"
        )
    }

    /// The env presence step of a non-guaranteed chain: `if let Some(v) =
    /// lookup { parse } else { miss }`, prefixed by the name prereq when the
    /// variable name is itself a non-guaranteed sibling.
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
            "{pre}if let Some(v) = {lookup} {{\n{parse}\n    {why} = String::new();\n}} else {{\n    {why} = {miss};\n}}",
            parse = indent(&parse, 1),
        )
    }

    /// A guaranteed chain as an if/else-if cascade ending in `@default`,
    /// relative to column zero.
    fn chain_guaranteed(&mut self, field: &EntryField, dest: &str) -> String {
        let mut out = String::new();
        let mut first = true;
        for source in &field.sources {
            match source {
                Source::With => {
                    let acc = self.arg_read(field);
                    out.push_str(&format!(
                        "{}if let Some(v) = {acc}.clone() {{\n    {dest} = v;\n}}",
                        if first { "" } else { " else " },
                    ));
                    first = false;
                }
                Source::Env(name) => {
                    let lookup = self.env_lookup(name);
                    let label = self.env_label(name);
                    let parse = self.env_parse(field, dest, &label);
                    out.push_str(&format!(
                        "{}if let Some(v) = {lookup} {{\n{parse}\n}}",
                        if first { "" } else { " else " },
                        parse = indent(&parse, 1),
                    ));
                    first = false;
                }
                Source::Default(v) => {
                    let lit = literal(&field.target, v, self.module);
                    if first {
                        out.push_str(&format!("{dest} = {lit};"));
                    } else {
                        out.push_str(&format!(" else {{\n    {dest} = {lit};\n}}"));
                    }
                    return out;
                }
                Source::Arg => {}
            }
        }
        out
    }

    /// The whole match reduces to a comparison of the Display-stringified
    /// subject against Display-stringified patterns (see the module doc):
    /// `switch_header`/`pattern_lit` cooperate on this, so neither needs the
    /// subject's declared type.
    fn switch_header(&self, subject: &str) -> String {
        format!("match ({subject}).to_string().as_str()")
    }

    fn case_open(&self, pattern: &str) -> String {
        format!("{pattern} => {{")
    }

    fn default_open(&self) -> String {
        "_ => {".to_string()
    }

    fn case_tail(&self) -> Option<&'static str> {
        None
    }

    fn case_close(&self) -> Option<&'static str> {
        Some("}")
    }

    fn pattern_lit(&self, pattern: &serde_json::Value) -> String {
        pattern_literal(pattern)
    }

    /// Every enum is open, so an undeclared value can still arrive at run
    /// time even with total declared coverage. Failing construction beats
    /// freezing a silent zero value into the resolved settings.
    fn select_miss(
        &mut self,
        field: &EntryField,
        subject_head: &str,
        subject_expr: &str,
        guaranteed: bool,
    ) -> Leaf {
        if guaranteed {
            Leaf(checks::config_error(&format!(
                "format!(\"{field}: match on {subject}: unmatched value {{}}\", {subject_expr})",
                field = field.name,
                subject = subject_head,
            )))
        } else {
            Leaf(format!(
                "{} = \"match: unmatched value\".to_string();",
                why_var(&field.name)
            ))
        }
    }

    /// The `@format` template rendered through `format!`, with the
    /// `@str::*` pipeline folded in, then cast to the field type. A
    /// `format!` template (rather than `+` concatenation) sidesteps Rust
    /// string-concatenation's ownership rules (`String + &str` only, never
    /// `&str + &str`), which a literal-led template would otherwise hit.
    /// Relative to column zero.
    fn format_assign(&mut self, field: &EntryField, dest: &str) -> String {
        let Some(parts) = field.format.clone() else {
            return String::new();
        };
        let mut template = String::new();
        let mut args: Vec<String> = Vec::new();
        for part in &parts {
            match part {
                TemplatePart::Lit(s) => template.push_str(&s.replace('{', "{{").replace('}', "}}")),
                TemplatePart::Field(p) => {
                    template.push_str("{}");
                    let t = self.path_type(p);
                    let expr = self.path_read(p);
                    args.push(self.to_string_expr(&expr, &t));
                }
                // An op-input placeholder cannot appear in a field template
                // (the frontend rejects it); render empty defensively.
                TemplatePart::Input(_) => {}
            }
        }
        let concat = if args.is_empty() {
            format!("{template:?}.to_string()")
        } else {
            format!("format!({template:?}, {})", args.join(", "))
        };
        let expr = apply_transforms(concat, &field.transforms, self.helpers, self.refs);
        format!("{dest} = {};", cast_string(&field.target, &expr))
    }

    /// The `@str::*` pipeline over an already-resolved destination, relative
    /// to column zero, or `None` when the field declares no transforms.
    fn transforms_body(&mut self, field: &EntryField, dest: &str) -> Option<String> {
        if field.transforms.is_empty() {
            return None;
        }
        let expr = self.to_string_expr(dest, &field.target);
        let expr = apply_transforms(expr, &field.transforms, self.helpers, self.refs);
        Some(format!("{dest} = {};", cast_string(&field.target, &expr)))
    }

    /// A structured source: an `@arg`/`@with` value passes typed; an env
    /// value probes its required members (missing/null is absence, mirroring
    /// Go's typed-unmarshal parity) before decoding strictly into the wire
    /// struct. Unlike Go/TypeScript's strict decode, this does not reject an
    /// unknown field (the struct's own derived `Deserialize` already
    /// tolerates one, and duplicating Go's manual `DisallowUnknownFields`
    /// strictness for an env-sourced construction value was judged not worth
    /// the extra decoder plumbing for this pass). Relative to column zero.
    fn structured_body(&mut self, field: &EntryField, shape: &Shape) -> String {
        let dest = self.ident(&field.name);
        if field.sources.iter().any(|s| matches!(s, Source::Arg)) {
            return format!("{dest} = {};", self.arg_read(field));
        }
        let why = why_var(&field.name);
        let mut out = format!("let mut {why} = \"no source\".to_string();\n");
        let ty = type_ident_from_id(&shape.id);
        let mut required_checks = String::new();
        if let ShapeKind::Structure { members, .. } = &shape.kind {
            for m in members.iter().filter(|m| m.required) {
                let name = wire_key(m);
                let fail = checks::config_error(&format!(
                    "format!(\"{{}}: missing field {name}\", label)"
                ));
                required_checks.push_str(&format!(
                    "if probe.get({name:?}).map(|v| v.is_null()).unwrap_or(true) {{\n    {fail}\n}}\n",
                ));
            }
        }
        let validate = if validation::shape_has_checks(shape) {
            "if let Err(e) = decoded.validate() {\n    return Err(TonoError::Validation(e));\n}\n"
                .to_string()
        } else {
            String::new()
        };

        let cascade = self.source_cascade(field, &dest, &why, |this, name| {
            let lookup = this.env_lookup(name);
            let label = this.env_label(name);
            let miss = this.env_miss_reason(name);
            let pre = this.env_name_prereq(name, &why);
            let fail_parse = checks::config_error("format!(\"{}: {}\", label, e)");
            format!(
                "{pre}if let Some(raw) = {lookup} {{\n    let label = {label};\n    let probe: serde_json::Value = match serde_json::from_str(&raw) {{\n        Ok(v) => v,\n        Err(e) => {{ {fail_parse} }}\n    }};\n{required}    let decoded: {ty} = match serde_json::from_str(&raw) {{\n        Ok(v) => v,\n        Err(e) => {{ {fail_parse} }}\n    }};\n{validate}    {dest} = decoded;\n    {why} = String::new();\n}} else {{\n    {why} = {miss};\n}}",
                required = indent(&required_checks, 1),
                validate = indent(&validate, 1),
            )
        });
        out.push_str(&cascade);
        out
    }

    /// A map/list field: an `@arg`/`@with` value passes typed, an env value
    /// decodes as JSON whole. Relative to column zero.
    fn json_body(&mut self, field: &EntryField) -> String {
        let dest = self.ident(&field.name);
        if field.sources.iter().any(|s| matches!(s, Source::Arg)) {
            return format!("{dest} = {};", self.arg_read(field));
        }
        let why = why_var(&field.name);
        let mut out = format!("let mut {why} = \"no source\".to_string();\n");
        let cascade = self.source_cascade(field, &dest, &why, |this, name| {
            let lookup = this.env_lookup(name);
            let label = this.env_label(name);
            let miss = this.env_miss_reason(name);
            let pre = this.env_name_prereq(name, &why);
            let fail = checks::config_error("format!(\"{}: {}\", label, e)");
            format!(
                "{pre}if let Some(raw) = {lookup} {{\n    let label = {label};\n    match serde_json::from_str(&raw) {{\n        Ok(v) => {{ {dest} = v; }}\n        Err(e) => {{ {fail} }}\n    }}\n    {why} = String::new();\n}} else {{\n    {why} = {miss};\n}}",
            )
        });
        out.push_str(&cascade);
        out
    }

    fn require_member(&mut self, head: &str, member: &str, leaf: &Tref, name: &str) -> String {
        let head_ident = field_snake_ren(
            head,
            self.entry.field_rename(head, LANG).as_deref(),
            self.config,
        );
        let member_ident = field_snake(member, self.config);
        let zero = zero_value(leaf, self.module, self.config);
        let msg = format!("{name}: no value");
        format!(
            "if s.{head_ident}.{member_ident} == {zero} {{\n    return Err(TonoError::Config(ConfigError {{ message: {msg:?}.to_string() }}));\n}}"
        )
    }

    fn require_member_deferred(&mut self, name: &str, why: &str) -> String {
        let w = why_var(why);
        format!(
            "if {w} != \"\" {{\n    return Err(TonoError::Config(ConfigError {{ message: format!(\"{name} <- {{}}\", {w}) }}));\n}}"
        )
    }

    fn require_string(&mut self, head: &str, target: &Tref) -> String {
        let ident = field_snake_ren(
            head,
            self.entry.field_rename(head, LANG).as_deref(),
            self.config,
        );
        let zero = zero_value(target, self.module, self.config);
        let w = why_var(head);
        format!(
            "if s.{ident} == {zero} {{\n    let reason = if {w}.is_empty() {{ \"no value\".to_string() }} else {{ {w}.clone() }};\n    return Err(TonoError::Config(ConfigError {{ message: format!(\"{head} <- {{}}\", reason) }}));\n}}"
        )
    }

    fn require_bytes(&mut self, head: &str) -> String {
        let ident = field_snake_ren(
            head,
            self.entry.field_rename(head, LANG).as_deref(),
            self.config,
        );
        let w = why_var(head);
        format!(
            "if s.{ident}.is_empty() {{\n    let reason = if {w}.is_empty() {{ \"no value\".to_string() }} else {{ {w}.clone() }};\n    return Err(TonoError::Config(ConfigError {{ message: format!(\"{head} <- {{}}\", reason) }}));\n}}"
        )
    }

    fn require_numeric(&mut self, head: &str, target: &Tref) -> String {
        let ident = field_snake_ren(
            head,
            self.entry.field_rename(head, LANG).as_deref(),
            self.config,
        );
        let w = why_var(head);
        let zero = numeric_zero(target);
        format!(
            "if !{w}.is_empty() && s.{ident} == {zero} {{\n    return Err(TonoError::Config(ConfigError {{ message: format!(\"{head} <- {{}}\", {w}) }}));\n}}"
        )
    }
}
