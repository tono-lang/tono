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

use super::resolve_call;
use super::resolve_env;
use super::resolve_requires;
use super::*;
use crate::codegen::entries::plan::{Cond, Emitter, Leaf};
use crate::ir::{EntryCall, EnvName};

/// The local variable a map-indexed match binds, scoped to the field driving
/// the match so two map matches in the same body never collide.
fn map_index_var(bound: &str) -> String {
    snake(&format!("{bound}_v"))
}

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
    /// Each top-level field's standalone resolver function (see
    /// [`Emitter::resolve_fn_call`]), collected here and flushed into the
    /// entry's own decls once every field is built.
    pub(super) resolve_fns: &'b mut Vec<Decl>,
    /// Entry-prefixes a resolver function's name in a multi-entry module,
    /// matching every other per-entry companion.
    pub(super) multi: bool,
}

/// An expression casting a Rust `String` into the field's target type. See
/// [`resolve_env::cast_string`] for the full leaf table.
fn cast_string(t: &Tref, expr: &str) -> String {
    resolve_env::cast_string(t, expr)
}

impl Resolver<'_, '_> {
    fn arg_read(&self, field: &EntryField) -> String {
        format!(
            "{}{}",
            self.arg_prefix,
            arg_snake(&field.name, &field.traits, self.lang())
        )
    }

    /// The name of a top-level field's standalone resolver function,
    /// entry-prefixed only in a multi-entry module (matching every other
    /// per-entry companion name).
    fn resolve_fn_name(&self, field: &EntryField) -> String {
        // "setting" disambiguates from the shared per-op helpers named after
        // a field-shaped concept directly (`resolve_max_retries`): a field
        // named `max_retries` would otherwise collide with the unrelated
        // retry-count helper.
        if self.multi {
            format!("resolve_setting_{}_{}", snake(self.entry.name), field.name)
        } else {
            format!("resolve_setting_{}", field.name)
        }
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

    /// A resolved value on its way into its construction slot: wrapped when
    /// that slot is stored as an `Option` (a foreign handle, which has no
    /// zero value in Rust -- see `ext::settings_field_type`), verbatim
    /// otherwise.
    fn store(&self, field: &EntryField, expr: &str) -> String {
        ext::wrap_stored(&field.target, self.module, expr)
    }

    /// The statements parsing a raw env string `v` into `dest`, by the
    /// field's declared type; a parse failure fails construction naming the
    /// variable (`label_expr`) and the type. Relative to column zero.
    fn env_parse(&mut self, field: &EntryField, dest: &str, label_expr: &str) -> String {
        resolve_env::env_parse(self, field, dest, label_expr)
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

    fn call_assign(&mut self, field: &EntryField, call: &EntryCall, dest: &str) -> String {
        resolve_call::call_assign(self, field, call, dest)
    }

    fn handle_call_assign(
        &mut self,
        field: &EntryField,
        call: &crate::ir::OpImplCall,
        dest: &str,
    ) -> String {
        super::ext::handle_call_assign(self, field, call, dest)
    }

    /// A call-sourced field's `@with` fallback only ever reaches this leaf
    /// from the builder path (an entry with a `@with`
    /// field always builds through `ClientBuilder`, never a bare `new`), so
    /// `arg_prefix` is always `"self."` here and the injected value reads
    /// off the builder's own `Option<T>` field.
    /// A foreign handle's own slot is not `Clone` in general (a connection,
    /// a pool, a provider typically is not), so its presence check and its
    /// assignment cannot independently re-read and unwrap the same
    /// `Option<T>` the way every other `@with` field type does below -- that
    /// would require a clone. Instead the check itself binds the value
    /// (`if let Some(v) = ..`), and [`Self::with_assign`] reads that same
    /// binding back rather than the field again: `build`/`with_*` consume
    /// `self` by value, so moving out of `self.<field>` here is sound, and
    /// clippy's own `unnecessary_unwrap` is exactly the check that would
    /// catch a regression back to the independent-reread shape.
    fn with_present_cond(&self, field: &EntryField) -> Cond {
        if ext::is_stored_wrapped(&field.target, self.module) {
            Cond(format!("let Some(v) = {}", self.arg_read(field)))
        } else {
            Cond(format!("{}.is_some()", self.arg_read(field)))
        }
    }

    /// `with_present_cond` is a plain boolean `if`, not an `if let`, for
    /// every field type but a foreign handle (see there), so this leaf
    /// independently re-reads and unwraps the same `Option<T>`; safe
    /// because the two always run together (the shared plan emits this leaf
    /// only inside the branch `with_present_cond` guards).
    fn with_assign(&self, field: &EntryField, dest: &str) -> Leaf {
        let value = if ext::is_stored_wrapped(&field.target, self.module) {
            "v".to_string()
        } else {
            format!("{}.unwrap()", self.arg_take(field))
        };
        Leaf(format!("{dest} = {};", self.store(field, &value)))
    }

    /// An `@arg` field's own guaranteed assignment. Overridden purely to
    /// route the value through [`Self::store`]; the spelling is otherwise
    /// the shared plan's own.
    fn assign_arg(&mut self, field: &EntryField, dest: &str) -> Leaf {
        let value = self.arg_ident(field);
        Leaf(format!("{dest} = {};", self.store(field, &value)))
    }

    /// The `decode_opening`/JSON-body `@arg` assignment (a foreign-handle
    /// field has no declared shape in `module.shapes` -- its type lives in
    /// `ext_libs` -- so it reaches `json_body`/`decode_opening`, not
    /// [`Self::assign_arg`]). Overridden for the same reason: the value's
    /// stored slot may be wrapped in `Option`.
    fn arg_assign(&mut self, field: &EntryField, dest: &str) -> String {
        let value = self.arg_ident(field);
        format!("{dest} = {};", self.store(field, &value))
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

    /// `HashMap::get` gives `Option<&V>`: `None` is the only spelling of
    /// "not there", so a stored empty-string value and a missing key can
    /// never be confused the way a bare zero-value check would confuse them.
    fn map_index_bind(&mut self, path: &[String], key_expr: &str, bound: &str) -> Leaf {
        Leaf(format!(
            "let {} = {}.get(&{key_expr});",
            map_index_var(bound),
            self.path_expr(path),
        ))
    }

    /// Shadows the bound `Option<&V>` with its unwrapped payload: idiomatic
    /// `if let`, not an `is_some()`/`unwrap()` pair a linter would flag as
    /// redundant (the switch that follows can only run once the binding is
    /// already known `Some`).
    fn map_index_present_cond(&self, bound: &str) -> Cond {
        let var = map_index_var(bound);
        Cond(format!("let Some({var}) = {var}"))
    }

    fn map_index_value_expr(&self, bound: &str) -> String {
        map_index_var(bound)
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
            "if let Some(v) = {acc} {{\n    {dest} = {v};\n    {err} = None;\n}} else {{\n    {err} = Some({miss});\n}}",
            v = self.store(field, "v"),
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

    /// A guaranteed chain as an if/else-if cascade ending in `@default`: each
    /// source's own `else` only runs when every higher-priority one already
    /// missed, so a lower-priority source is never even evaluated (its parse
    /// never runs, its failure mode never fires) once an earlier one wins.
    /// That short-circuiting is why this stays a cascade rather than a
    /// set-flag sequence: a flag would still let every source run and only
    /// gate the final assignment, so a malformed but shadowed `@env` value
    /// would fail construction it does not fail today.
    fn chain_guaranteed(&mut self, field: &EntryField, dest: &str) -> String {
        let mut out = String::new();
        let mut first = true;
        for source in &field.sources {
            match source {
                Source::With => {
                    let acc = self.arg_take(field);
                    out.push_str(&format!(
                        "{}if let Some(v) = {acc} {{\n    {dest} = {v};\n}}",
                        if first { "" } else { " else " },
                        v = self.store(field, "v"),
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

    /// A top-level guaranteed field's chain as a standalone function: each
    /// source is an unconditional early-return guard in priority order, so
    /// there is no `else` to spell (a later guard is simply never reached
    /// once an earlier one returns) and no set-flag either. `@with` reads off
    /// a function parameter, not the builder's own field, so the function has
    /// no free variable tying it to one call site. Every import a leaf
    /// spelling pulls in (`env_parse`, via `self.refs`) lands on this
    /// function's own declaration, not the constructor's, since it is the
    /// only place the import is actually used.
    fn resolve_fn_call(&mut self, field: &EntryField, dest: &str) -> String {
        let name = self.resolve_fn_name(field);
        let has_with = field.sources.iter().any(|s| matches!(s, Source::With));
        let ty = rust_type(&field.target);
        let before = self.refs.len();
        let mut body = String::new();
        for source in &field.sources {
            match source {
                Source::With => {
                    body.push_str("if let Some(v) = with {\n    return v;\n}\n");
                }
                Source::Env(name) => {
                    let lookup = self.env_lookup(name);
                    let label = self.env_label(name);
                    let parse = self.env_parse(field, "parsed", &label);
                    body.push_str(&format!(
                        "if let Some(v) = {lookup} {{\n    let parsed: {ty};\n{parse}\n    return parsed;\n}}\n",
                        parse = indent(&parse, 1),
                    ));
                }
                Source::Default(v) => {
                    body.push_str(&format!("{}\n", literal(&field.target, v, self.module)));
                }
                Source::Arg => {}
            }
        }
        let fn_refs: Vec<Symbol> = self.refs.split_off(before);
        let param = if has_with {
            format!("with: Option<{ty}>")
        } else {
            String::new()
        };
        let decl = Decl::raw_with(
            format!(
                "/// Resolves the {field} construction value.\nfn {name}({param}) -> {ty} {{\n{body}}}",
                field = field.name,
                body = indent(&body, 1),
            ),
            fn_refs,
        );
        // `resolution_body` runs twice per entry (the builder's `self.`-prefixed
        // reads and the plain `new`'s bare ones), but only one of the two ever
        // makes it into the file; both share the same `resolve_fns` sink, so
        // the identical second declaration is dropped rather than duplicated.
        if !self.resolve_fns.contains(&decl) {
            self.resolve_fns.push(decl);
        }
        let arg = if has_with {
            self.arg_take(field)
        } else {
            String::new()
        };
        format!("{dest} = {};", self.store(field, &format!("{name}({arg})")))
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
        resolve_requires::require_member(self, head, member, leaf, name)
    }

    fn require_member_deferred(&mut self, name: &str, err: &str) -> String {
        resolve_requires::require_member_deferred(name, err)
    }

    fn require_string(&mut self, head: &str, target: &Tref) -> String {
        resolve_requires::require_string(self, head, target)
    }

    fn require_bytes(&mut self, head: &str) -> String {
        resolve_requires::require_bytes(self, head)
    }

    fn require_numeric(&mut self, head: &str, target: &Tref) -> String {
        resolve_requires::require_numeric(self, head, target)
    }
}
