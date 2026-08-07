//! The Rust leaf spellings for the shared resolution plan.
//!
//! The control flow (source ordering, absence tracking, config composition)
//! lives once in `codegen::entries::plan`; this file only says how each leaf
//! statement reads in Rust. Two adaptations run throughout:
//!
//! - every `<field>_err` error variable is `Option<ConfigError>` (`None`
//!   while unresolved), read through `.is_some()`/`.is_none()` rather than an
//!   equality comparison — `ConfigError` carries no `PartialEq` (nothing
//!   forces one), so the shared `cond_err_present`/`cond_err_absent` defaults
//!   Go/TypeScript use (`!= nil`/`!== undefined`) do not fit here, and
//!   `wrap_from` reads the head's message through `.as_ref()` rather than
//!   moving or cloning it, so the same field can still be read (or wrapped
//!   again) by a later consumer;
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
    fn arg_read(&self, field: &EntryField) -> String {
        format!(
            "{}{}",
            self.arg_prefix,
            arg_snake(&field.name, &field.traits, self.lang())
        )
    }

    /// The `@with` accessor as an owned `Option`: a `Copy`-typed option
    /// reads bare (a `.clone()` on it trips the generated SDK's
    /// clone-on-copy lint), everything else clones out of the borrow.
    fn arg_take(&self, field: &EntryField) -> String {
        let acc = self.arg_read(field);
        if matches!(
            field.target,
            Tref::Prim(
                Prim::Bool
                    | Prim::I8
                    | Prim::I16
                    | Prim::I32
                    | Prim::I64
                    | Prim::U8
                    | Prim::U16
                    | Prim::U32
                    | Prim::U64
                    | Prim::Float
            )
        ) {
            acc
        } else {
            format!("{acc}.clone()")
        }
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
    /// around it (folding the steps into `if {err}.is_some() { ... }` guards)
    /// is written once. `env_step` takes `&mut Self` explicitly rather than
    /// capturing it, so it can still call the `&mut self` env helpers while
    /// this method holds its own `&mut self` borrow across the loop.
    fn source_cascade(
        &mut self,
        field: &EntryField,
        dest: &str,
        err: &str,
        mut env_step: impl FnMut(&mut Self, &EnvName) -> String,
    ) -> String {
        let mut steps: Vec<String> = Vec::new();
        for source in &field.sources {
            match source {
                Source::With => steps.push(self.with_step_body(field, dest, err)),
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
                    format!("if {err}.is_some() {{\n{}\n}}", indent(step, 1))
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

    fn err_ident(&self, field_name: &str) -> String {
        err_var(field_name)
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
        // A literal name is already `&'static str` and passes bare; a
        // dynamic name is a `String`-typed expression (`to_string_expr`'s
        // output) and borrows into the `&str` parameter. The literal case
        // must not borrow again: `&&str` coerces, but trips the generated
        // SDK's needless-borrow lint.
        if name_expr.starts_with('"') {
            format!("{}({name_expr})", shared_slot("read_env"))
        } else {
            format!("{}(&{name_expr})", shared_slot("read_env"))
        }
    }

    fn env_miss_error(&mut self, name: &EnvName) -> String {
        let message = match name {
            EnvName::Name(n) => format!("{:?}.to_string()", format!("env {n}: empty")),
            EnvName::Field(fr) => {
                let t = self.path_type_of(&fr.field);
                let read = self.path_read(&fr.field);
                let s = self.to_string_expr(&read, &t);
                format!("format!(\"env {{}}: empty\", {s})")
            }
        };
        format!("Some({})", self.config_error_expr(&message, None))
    }

    fn err_open(&self, field_name: &str) -> Leaf {
        Leaf(format!(
            "let mut {}: Option<ConfigError> = None;",
            err_var(field_name)
        ))
    }

    fn absent_literal(&self) -> &'static str {
        "None"
    }

    fn cond_err_present(&self, field_name: &str) -> Cond {
        Cond(format!("{}.is_some()", self.err_ident(field_name)))
    }

    fn cond_err_absent(&self, field_name: &str) -> Cond {
        Cond(format!("{}.is_none()", self.err_ident(field_name)))
    }

    fn config_error_expr(&self, message_expr: &str, _cause: Option<&str>) -> String {
        // Rust's ConfigError carries no boxed cause: the chain of "who
        // consumed what" is already whole in the message text, and a wrapped
        // step reads the head's message through a borrow (see `wrap_from`),
        // never moves or clones the head's own error value.
        format!("ConfigError {{ message: {message_expr} }}")
    }

    /// Borrows the head's message rather than moving or cloning it
    /// (`ConfigError` has no `PartialEq`/`Clone` — its would-be cause is a
    /// `Box<dyn Error>` — and the head may still be read by another consumer
    /// later).
    fn wrap_error_expr(&self, head: &str, head_err: &str) -> String {
        format!("{head_err}.as_ref().map(|e| ConfigError {{ message: format!(\"{head} <- {{}}\", e.message) }})")
    }

    fn field_guaranteed(&self, name: &str) -> bool {
        self.entry.field_guaranteed(name)
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
    fn with_step_body(&self, field: &EntryField, dest: &str, err: &str) -> String {
        let acc = self.arg_take(field);
        format!(
            "if let Some(v) = {acc} {{\n    {dest} = v;\n    {err} = None;\n}} else {{\n    {err} = Some({miss});\n}}",
            miss = self.config_error_expr("\"not configured\".to_string()", None),
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
        err: &str,
    ) -> String {
        let (lookup, label, miss, pre) = plan::env_parts(self, name, err);
        let parse = self.env_parse(field, dest, &label);
        format!(
            "{pre}if let Some(v) = {lookup} {{\n{parse}\n    {err} = None;\n}} else {{\n    {err} = {miss};\n}}",
            parse = indent(&parse, 1),
        )
    }

    /// A guaranteed chain, spelled with a set-flag so every env variable is
    /// read exactly once and `@default` closes the chain, matching every
    /// other target's spelling (see [`Emitter::chain_guaranteed`]). Relative
    /// to column zero.
    fn chain_guaranteed(&mut self, field: &EntryField, dest: &str) -> String {
        // A chain that is just the default needs no flag.
        if let [Source::Default(v)] = field.sources.as_slice() {
            return format!("{dest} = {};", literal(&field.target, v, self.module));
        }
        let flag = format!("{}_set", field.name);
        let mut out = format!("let mut {flag} = false;");
        for source in &field.sources {
            match source {
                Source::With => {
                    let acc = self.arg_take(field);
                    out.push_str(&format!(
                        "\nif !{flag} {{\n    if let Some(v) = {acc} {{\n        {dest} = v;\n        {flag} = true;\n    }}\n}}",
                    ));
                }
                Source::Env(name) => {
                    let lookup = self.env_lookup(name);
                    let label = self.env_label(name);
                    let parse = self.env_parse(field, dest, &label);
                    out.push_str(&format!(
                        "\nif !{flag} {{\n    if let Some(v) = {lookup} {{\n{parse}\n        {flag} = true;\n    }}\n}}",
                        parse = indent(&parse, 2),
                    ));
                }
                Source::Default(v) => {
                    out.push_str(&format!(
                        "\nif !{flag} {{\n    {dest} = {};\n}}",
                        literal(&field.target, v, self.module),
                    ));
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
                "{} = Some({});",
                err_var(&field.name),
                self.config_error_expr("\"match: unmatched value\".to_string()", None),
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
                // An op-input or op-parameter placeholder cannot appear in a
                // field template (the frontend rejects it); render empty
                // defensively.
                TemplatePart::Input(_) | TemplatePart::Param(_) => {}
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
        let mut out = String::new();
        let Some((dest, err)) = plan::decode_opening(self, field, &mut out) else {
            return out;
        };
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

        let cascade = self.source_cascade(field, &dest, &err, |this, name| {
            let (lookup, label, miss, pre) = plan::env_parts(this, name, &err);
            let fail_parse = checks::config_error("format!(\"{}: {}\", label, e)");
            format!(
                "{pre}if let Some(raw) = {lookup} {{\n    let label = {label};\n    let probe: serde_json::Value = match serde_json::from_str(&raw) {{\n        Ok(v) => v,\n        Err(e) => {{ {fail_parse} }}\n    }};\n{required}    let decoded: {ty} = match serde_json::from_str(&raw) {{\n        Ok(v) => v,\n        Err(e) => {{ {fail_parse} }}\n    }};\n{validate}    {dest} = decoded;\n    {err} = None;\n}} else {{\n    {err} = {miss};\n}}",
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
        let mut out = String::new();
        let Some((dest, err)) = plan::decode_opening(self, field, &mut out) else {
            return out;
        };
        let cascade = self.source_cascade(field, &dest, &err, |this, name| {
            let (lookup, label, miss, pre) = plan::env_parts(this, name, &err);
            let fail = checks::config_error("format!(\"{}: {}\", label, e)");
            format!(
                "{pre}if let Some(raw) = {lookup} {{\n    let label = {label};\n    match serde_json::from_str(&raw) {{\n        Ok(v) => {{ {dest} = v; }}\n        Err(e) => {{ {fail} }}\n    }}\n    {err} = None;\n}} else {{\n    {err} = {miss};\n}}",
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

    fn require_member_deferred(&mut self, name: &str, err: &str) -> String {
        let e = err_var(err);
        format!(
            "if let Some({e}) = &{e} {{\n    return Err(TonoError::Config(ConfigError {{ message: format!(\"{name} <- {{}}\", {e}.message) }}));\n}}"
        )
    }

    fn require_string(&mut self, head: &str, target: &Tref) -> String {
        let ident = field_snake_ren(
            head,
            self.entry.field_rename(head, LANG).as_deref(),
            self.config,
        );
        let zero = zero_value(target, self.module, self.config);
        let e = err_var(head);
        format!(
            "if s.{ident} == {zero} {{\n    let reason = {e}.as_ref().map(|err| err.message.clone()).unwrap_or_else(|| \"no value\".to_string());\n    return Err(TonoError::Config(ConfigError {{ message: format!(\"{head} <- {{}}\", reason) }}));\n}}"
        )
    }

    fn require_bytes(&mut self, head: &str) -> String {
        let ident = field_snake_ren(
            head,
            self.entry.field_rename(head, LANG).as_deref(),
            self.config,
        );
        let e = err_var(head);
        format!(
            "if s.{ident}.is_empty() {{\n    let reason = {e}.as_ref().map(|err| err.message.clone()).unwrap_or_else(|| \"no value\".to_string());\n    return Err(TonoError::Config(ConfigError {{ message: format!(\"{head} <- {{}}\", reason) }}));\n}}"
        )
    }

    fn require_numeric(&mut self, head: &str, target: &Tref) -> String {
        let ident = field_snake_ren(
            head,
            self.entry.field_rename(head, LANG).as_deref(),
            self.config,
        );
        let e = err_var(head);
        let zero = numeric_zero(target);
        format!(
            "if let Some({e}) = &{e} {{\n    if s.{ident} == {zero} {{\n        return Err(TonoError::Config(ConfigError {{ message: format!(\"{head} <- {{}}\", {e}.message) }}));\n    }}\n}}"
        )
    }
}
