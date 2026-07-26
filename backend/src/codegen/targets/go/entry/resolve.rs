//! The per-field resolution emitter (split from the entry module to
//! keep files within the size ceiling; same module surface).

use super::*;

/// The per-field resolution emitter: one field, one block of statements, in
/// the field's own idiom (scalar chain, switch, strict decode, composition).
pub(super) struct Resolver<'a, 'b> {
    pub(super) entry: &'a EntryModel<'a>,
    pub(super) module: &'a Module,
    pub(super) config: &'a CasingConfig,
    pub(super) helpers: &'b mut Helpers,
    pub(super) refs: &'b mut Vec<Symbol>,
    pub(super) body: &'b mut String,
}

impl Resolver<'_, '_> {
    fn push(&mut self, s: &str) {
        self.body.push_str(s);
    }

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
        let head = self
            .entry
            .fields
            .iter()
            .find(|f| path.first().is_some_and(|h| *h == f.name));
        let Some(head) = head else {
            return Tref::Prim(Prim::String);
        };
        if path.len() == 1 {
            return head.target.clone();
        }
        if let Tref::Ref { id, .. } = &head.target {
            if let Some(shape) = self.module.shapes.iter().find(|s| s.id == *id) {
                let target = match &shape.kind {
                    ShapeKind::Config { fields } => fields
                        .iter()
                        .find(|f| f.name == path[1])
                        .map(|f| f.target.clone()),
                    ShapeKind::Structure { members, .. } => members
                        .iter()
                        .find(|m| m.name == path[1])
                        .map(|m| m.target.clone()),
                    _ => None,
                };
                if let Some(t) = target {
                    return t;
                }
            }
        }
        Tref::Prim(Prim::String)
    }

    fn guaranteed(&self, name: &str) -> bool {
        self.entry
            .fields
            .iter()
            .find(|f| f.name == name)
            .is_some_and(|f| self.entry.is_guaranteed(f))
    }

    pub(super) fn emit_field(&mut self, field: &EntryField) {
        match self.entry.field_shape(field, self.module) {
            FieldShape::Config(shape) => self.emit_config(field, shape),
            FieldShape::Structured(shape) => self.emit_structured(field, shape),
            FieldShape::Json => self.emit_json(field),
            FieldShape::Scalar => self.emit_scalar(field),
        }
    }

    /// A scalar field: an explicit `@arg`, a match selection, a `@format`
    /// derivation, or a plain source chain.
    fn emit_scalar(&mut self, field: &EntryField) {
        if field.sources.iter().any(|s| matches!(s, Source::Arg)) {
            let assign = format!("\t{} = {}\n", self.ident(&field.name), camel(&field.name));
            self.push(&assign);
            return;
        }
        if field.select.is_some() {
            self.emit_select(field);
            return;
        }
        if field.format.is_some() {
            self.emit_format(field);
            return;
        }
        self.emit_chain(field);
    }

    /// The plain source chain of one field, written to its Settings slot.
    fn emit_chain(&mut self, field: &EntryField) {
        let dest = self.ident(&field.name);
        if self.entry.is_guaranteed(field) {
            let stmts = self.chain_cascade(field, &dest);
            self.push(&stmts);
        } else {
            let why = why_var(&field.name);
            let opener = format!("\t{why} := \"no source\"\n");
            self.push(&opener);
            let stmts = self.chain_sequential(field, &dest, &why);
            self.push(&stmts);
        }
    }

    /// A guaranteed chain as an if/else cascade ending in `@default`.
    fn chain_cascade(&mut self, field: &EntryField, dest: &str) -> String {
        let mut out = String::new();
        let mut first = true;
        for source in &field.sources {
            match source {
                Source::With => {
                    out.push_str(&format!(
                        "{}if w.{carrier} != nil {{\n\t\t{dest} = *w.{carrier}\n\t}}",
                        if first { "\t" } else { " else " },
                        carrier = camel(&field.name),
                    ));
                    first = false;
                }
                Source::Env(name) => {
                    let lookup = self.env_lookup(name);
                    let label = self.env_label(name);
                    out.push_str(&format!(
                        "{}if v, ok := {lookup}; ok && v != \"\" {{\n{parse}\t}}",
                        if first { "\t" } else { " else " },
                        parse = self.env_parse(field, dest, &label),
                    ));
                    first = false;
                }
                Source::Default(v) => {
                    let lit = literal(&field.target, v);
                    if first {
                        out.push_str(&format!("\t{dest} = {lit}\n"));
                    } else {
                        out.push_str(&format!(" else {{\n\t\t{dest} = {lit}\n\t}}\n"));
                    }
                    return out;
                }
                Source::Arg => {}
            }
        }
        if !first {
            out.push('\n');
        }
        out
    }

    /// A non-guaranteed chain as sequential "still absent" steps carrying the
    /// why-reason of the last source tried.
    fn chain_sequential(&mut self, field: &EntryField, dest: &str, why: &str) -> String {
        let mut out = String::new();
        let mut first = true;
        for source in &field.sources {
            let step = match source {
                Source::With => format!(
                    "if w.{carrier} != nil {{\n\t\t{dest} = *w.{carrier}\n\t\t{why} = \"\"\n\t}} else {{\n\t\t{why} = \"not configured\"\n\t}}\n",
                    carrier = camel(&field.name),
                ),
                Source::Env(name) => {
                    let lookup = self.env_lookup(name);
                    let label = self.env_label(name);
                    let miss = self.env_miss_reason(name);
                    let pre = self.env_name_prereq(name, why);
                    // The prereq ends in "else ", chaining straight into
                    // this if: the whole run is one balanced if/else-if/else.
                    // env_parse already speaks at if-body depth; the wrapper
                    // below indents the whole step when it is not the first.
                    format!(
                        "{pre}if v, ok := {lookup}; ok && v != \"\" {{\n{parse}\t\t{why} = \"\"\n\t}} else {{\n\t\t{why} = {miss}\n\t}}\n",
                        parse = self.env_parse(field, dest, &label),
                    )
                }
                Source::Default(v) => format!(
                    "{dest} = {lit}\n\t{why} = \"\"\n",
                    lit = literal(&field.target, v),
                ),
                Source::Arg => continue,
            };
            if first {
                out.push_str(&format!("\t{step}"));
                first = false;
            } else {
                out.push_str(&format!(
                    "\tif {why} != \"\" {{\n{indented}\t}}\n",
                    indented = indent(step.trim_end_matches('\n')),
                ));
            }
        }
        out
    }

    /// The prereq guard when the env variable's own name comes from a sibling
    /// field that may itself be absent. Returns the opened guard text (the
    /// caller closes it).
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
            "if {head_why} != \"\" {{\n\t\t{why} = \"{head} <- \" + {head_why}\n\t}} else ",
            head_why = why_var(head),
            head = head,
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

    /// The label naming the variable in a parse error.
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

    /// The statements parsing a raw env string `v` into the destination, by
    /// the field's declared type; a parse failure fails construction naming
    /// the variable and the type.
    fn env_parse(&mut self, field: &EntryField, dest: &str, label: &str) -> String {
        let t = &field.target;
        match t {
            Tref::Prim(Prim::Bool) => {
                self.import("fmt", "fmt");
                format!(
                    "\t\tswitch v {{\n\t\tcase \"true\", \"1\":\n\t\t\t{dest} = true\n\t\tcase \"false\", \"0\":\n\t\t\t{dest} = false\n\t\tdefault:\n\t\t\treturn nil, fmt.Errorf(\"%s: invalid bool %q (want true/false/1/0)\", {label}, v)\n\t\t}}\n"
                )
            }
            Tref::Prim(p @ (Prim::I8 | Prim::I16 | Prim::I32 | Prim::I64)) => {
                self.import("strconv", "strconv");
                self.import("fmt", "fmt");
                let bits = int_bits(p);
                format!(
                    "\t\tn, err := strconv.ParseInt(v, 10, {bits})\n\t\tif err != nil {{\n\t\t\treturn nil, fmt.Errorf(\"%s: invalid {prim} %q\", {label}, v)\n\t\t}}\n\t\t{dest} = {cast}(n)\n",
                    prim = prim_name(p),
                    cast = prim_spelling(p).go,
                )
            }
            Tref::Prim(p @ (Prim::U8 | Prim::U16 | Prim::U32 | Prim::U64)) => {
                self.import("strconv", "strconv");
                self.import("fmt", "fmt");
                let bits = int_bits(p);
                format!(
                    "\t\tn, err := strconv.ParseUint(v, 10, {bits})\n\t\tif err != nil {{\n\t\t\treturn nil, fmt.Errorf(\"%s: invalid {prim} %q\", {label}, v)\n\t\t}}\n\t\t{dest} = {cast}(n)\n",
                    prim = prim_name(p),
                    cast = prim_spelling(p).go,
                )
            }
            Tref::Prim(Prim::Float) => {
                self.import("strconv", "strconv");
                self.import("fmt", "fmt");
                format!(
                    "\t\tn, err := strconv.ParseFloat(v, 64)\n\t\tif err != nil {{\n\t\t\treturn nil, fmt.Errorf(\"%s: invalid float %q\", {label}, v)\n\t\t}}\n\t\t{dest} = n\n"
                )
            }
            Tref::Prim(Prim::Duration) => {
                self.import("time", "time");
                self.import("fmt", "fmt");
                format!(
                    "\t\tif _, err := time.ParseDuration(v); err != nil {{\n\t\t\treturn nil, fmt.Errorf(\"%s: invalid duration %q\", {label}, v)\n\t\t}}\n\t\t{dest} = Duration(v)\n"
                )
            }
            _ => format!("\t\t{dest} = {}\n", cast_string(t, "v")),
        }
    }

    /// `field: T = match .subject { ... }` lowered to the native switch.
    fn emit_select(&mut self, field: &EntryField) {
        let Some(select) = field.select.clone() else {
            return;
        };
        let dest = self.ident(&field.name);
        let guaranteed = self.entry.is_guaranteed(field);
        let why = why_var(&field.name);
        if !guaranteed {
            self.push(&format!("\t{why} := \"\"\n"));
        }
        let subject_head = select.subject.first().cloned().unwrap_or_default();
        let subject_expr = self.path_expr(&select.subject);
        let mut arms = String::new();
        let mut saw_wildcard = false;
        for arm in &select.arms {
            let stmts = self.arm_stmts(field, &arm.value, &dest, &why, guaranteed);
            match &arm.pattern {
                Some(p) => arms.push_str(&format!(
                    "\tcase {}:\n{}",
                    pattern_literal(p),
                    indent(stmts.trim_end_matches('\n'))
                )),
                None => {
                    saw_wildcard = true;
                    arms.push_str(&format!(
                        "\tdefault:\n{}",
                        indent(stmts.trim_end_matches('\n'))
                    ));
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
                    "\t\treturn nil, fmt.Errorf(\"{field}: match on {subject}: unmatched value %v\", {subject_expr})\n",
                    field = field.name,
                    subject = subject_head,
                )
            } else {
                format!("\t\t{why} = \"match: unmatched value\"\n")
            };
            arms.push_str(&format!("\tdefault:\n{miss}"));
        }
        let switch = format!("\tswitch {subject_expr} {{\n{arms}\t}}\n");
        if !self.guaranteed(&subject_head) {
            self.push(&format!(
                "\tif {subj_why} != \"\" {{\n\t\t{why} = \"{subject_head} <- \" + {subj_why}\n\t}} else {{\n{sw}\t}}\n",
                subj_why = why_var(&subject_head),
                sw = indent(switch.trim_end_matches('\n')),
            ));
        } else {
            self.push(&switch);
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
            ArmValue::Lit(v) => format!("\t{dest} = {}\n", literal(&field.target, v)),
            ArmValue::Field(path) => {
                let head = path.first().cloned().unwrap_or_default();
                let expr = self.path_expr(path);
                if self.guaranteed(&head) {
                    format!("\t{dest} = {expr}\n")
                } else {
                    format!(
                        "\tif {head_why} != \"\" {{\n\t\t{why} = \"{head} <- \" + {head_why}\n\t}} else {{\n\t\t{dest} = {expr}\n\t}}\n",
                        head_why = why_var(&head),
                    )
                }
            }
            ArmValue::Sources(sources) => {
                let stub = EntryField {
                    name: field.name.clone(),
                    target: field.target.clone(),
                    sources: sources.clone(),
                    format: None,
                    transforms: vec![],
                    select: None,
                    binds: vec![],
                    constraints: vec![],
                    traits: vec![],
                };
                if guaranteed {
                    self.chain_cascade(&stub, dest)
                } else {
                    format!(
                        "\t{why} = \"no source\"\n{}",
                        self.chain_sequential(&stub, dest, why)
                    )
                }
            }
        }
    }

    /// `@format` template plus the `@str::*` transform pipeline.
    fn emit_format(&mut self, field: &EntryField) {
        let Some(format_parts) = field.format.clone() else {
            return;
        };
        let dest = self.ident(&field.name);
        let mut concat: Vec<String> = Vec::new();
        let mut absent_deps: Vec<String> = Vec::new();
        for part in &format_parts {
            match part {
                TemplatePart::Lit(s) => concat.push(format!("{s:?}")),
                TemplatePart::Field(p) => {
                    let head = p.first().cloned().unwrap_or_default();
                    if !self.guaranteed(&head) && !absent_deps.contains(&head) {
                        absent_deps.push(head.clone());
                    }
                    let t = self.path_type(p);
                    let expr = self.path_expr(p);
                    concat.push(self.as_string_expr(&expr, &t));
                }
                // An op-input placeholder cannot appear in a field template;
                // the frontend rejects it. Render empty defensively.
                TemplatePart::Input(_) => concat.push("\"\"".to_string()),
            }
        }
        if !field.transforms.is_empty() {
            self.import("strings", "strings");
        }
        let expr = apply_transforms(concat.join(" + "), &field.transforms, self.helpers);
        let assign = format!("\t{dest} = {}\n", cast_string(&field.target, &expr));
        if absent_deps.is_empty() {
            self.push(&assign);
            return;
        }
        let why = why_var(&field.name);
        let mut out = format!("\t{why} := \"\"\n");
        let mut chain = String::new();
        for (i, dep) in absent_deps.iter().enumerate() {
            chain.push_str(&format!(
                "{}if {dep_why} != \"\" {{\n\t\t{why} = \"{dep} <- \" + {dep_why}\n\t}}",
                if i == 0 { "\t" } else { " else " },
                dep_why = why_var(dep),
            ));
        }
        chain.push_str(&format!(" else {{\n\t{assign}\t}}\n"));
        out.push_str(&chain);
        self.push(&out);
    }

    /// A structured source: an explicit `@arg`/`@with` value passes typed, a
    /// JSON env value decodes strictly into the wire struct (required members
    /// checked by name, unknown fields rejected), and declared validation runs
    /// at construction. The error carries the variable's name as context.
    fn emit_structured(&mut self, field: &EntryField, shape: &Shape) {
        let dest = self.ident(&field.name);
        if field.sources.iter().any(|s| matches!(s, Source::Arg)) {
            let assign = format!("\t{dest} = {}\n", camel(&field.name));
            self.push(&assign);
            return;
        }
        // Without @arg a structured field can never be guaranteed (@default
        // does not apply to it), so the chain is always why-tracked.
        let why = why_var(&field.name);
        self.push(&format!("\t{why} := \"no source\"\n"));
        if field.sources.iter().any(|s| matches!(s, Source::With)) {
            let step = format!(
                "\tif w.{carrier} != nil {{\n\t\t{dest} = *w.{carrier}\n\t\t{why} = \"\"\n\t}} else {{\n\t\t{why} = \"not configured\"\n\t}}\n",
                carrier = camel(&field.name),
            );
            self.push(&step);
        }
        let Some(Source::Env(name)) = field.sources.iter().find(|s| matches!(s, Source::Env(_)))
        else {
            return;
        };
        self.import("os", "os");
        self.import("json", "encoding/json");
        self.import("fmt", "fmt");
        self.import("bytes", "bytes");
        let lookup = self.env_lookup(name);
        let label = self.env_label(name);
        let miss = self.env_miss_reason(name);
        let ty = type_ident_from_id(&shape.id);
        let mut required_checks = String::new();
        if let ShapeKind::Structure { members, .. } = &shape.kind {
            for m in members.iter().filter(|m| m.required) {
                required_checks.push_str(&format!(
                    "\t\tif _, ok := probe[{name:?}]; !ok {{\n\t\t\treturn nil, fmt.Errorf(\"%s: missing field {name}\", {label})\n\t\t}}\n",
                    name = m.name,
                ));
            }
        }
        let validate = if validation::shape_has_checks(shape) {
            let en = error_names();
            format!(
                "\t\tif vs := Validate{ty}(decoded); len(vs) > 0 {{\n\t\t\treturn nil, &{validation}{{Violations: vs}}\n\t\t}}\n",
                validation = en.validation,
            )
        } else {
            String::new()
        };
        let block = format!(
            "\tif raw, ok := {lookup}; ok && raw != \"\" {{\n\
             \t\tvar probe map[string]json.RawMessage\n\
             \t\tif err := json.Unmarshal([]byte(raw), &probe); err != nil {{\n\t\t\treturn nil, fmt.Errorf(\"%s: %v\", {label}, err)\n\t\t}}\n\
             {required_checks}\
             \t\tdec := json.NewDecoder(bytes.NewReader([]byte(raw)))\n\
             \t\tdec.DisallowUnknownFields()\n\
             \t\tvar decoded {ty}\n\
             \t\tif err := dec.Decode(&decoded); err != nil {{\n\t\t\treturn nil, fmt.Errorf(\"%s: %v\", {label}, err)\n\t\t}}\n\
             {validate}\
             \t\t{dest} = decoded\n\
             \t\t{why} = \"\"\n\
             \t}} else {{\n\t\t{why} = {miss}\n\t}}\n"
        );
        // An explicit @with value wins: the decode runs only while unset.
        self.push(&format!(
            "\tif {why} != \"\" {{\n{inner}\t}}\n",
            inner = indent(block.trim_end_matches('\n')),
        ));
    }

    /// A map/list field: an explicit `@arg`/`@with` value passes typed, an env
    /// value decodes as JSON whole.
    fn emit_json(&mut self, field: &EntryField) {
        let dest = self.ident(&field.name);
        if field.sources.iter().any(|s| matches!(s, Source::Arg)) {
            let assign = format!("\t{dest} = {}\n", camel(&field.name));
            self.push(&assign);
            return;
        }
        let why = why_var(&field.name);
        self.push(&format!("\t{why} := \"no source\"\n"));
        if field.sources.iter().any(|s| matches!(s, Source::With)) {
            let step = format!(
                "\tif w.{carrier} != nil {{\n\t\t{dest} = *w.{carrier}\n\t\t{why} = \"\"\n\t}} else {{\n\t\t{why} = \"not configured\"\n\t}}\n",
                carrier = camel(&field.name),
            );
            self.push(&step);
        }
        let Some(Source::Env(name)) = field.sources.iter().find(|s| matches!(s, Source::Env(_)))
        else {
            return;
        };
        self.import("os", "os");
        self.import("json", "encoding/json");
        self.import("fmt", "fmt");
        let lookup = self.env_lookup(name);
        let label = self.env_label(name);
        let miss = self.env_miss_reason(name);
        let block = format!(
            "\tif raw, ok := {lookup}; ok && raw != \"\" {{\n\
             \t\tif err := json.Unmarshal([]byte(raw), &{dest}); err != nil {{\n\t\t\treturn nil, fmt.Errorf(\"%s: %v\", {label}, err)\n\t\t}}\n\
             \t\t{why} = \"\"\n\
             \t}} else {{\n\t\t{why} = {miss}\n\t}}\n"
        );
        self.push(&format!(
            "\tif {why} != \"\" {{\n{inner}\t}}\n",
            inner = indent(block.trim_end_matches('\n')),
        ));
    }

    /// A composed config: per member, an entry `@bind` wins over the member's
    /// own sources.
    fn emit_config(&mut self, field: &EntryField, shape: &Shape) {
        let ShapeKind::Config { fields } = &shape.kind else {
            return;
        };
        let ty = type_ident_from_id(&shape.id);
        let dest = self.ident(&field.name);
        let mut block = format!("\t{{\n\t\tvar composed {ty}\n");
        for member in fields {
            let member_dest = format!("composed.{}", field_pascal(&member.name, self.config));
            let bind = field.binds.iter().find(|b| b.field == member.name);
            let mut member_stmts = String::new();
            if let Some(bind) = bind {
                let head = bind.source.first().cloned().unwrap_or_default();
                let expr = self.path_expr(&bind.source);
                if self.guaranteed(&head) {
                    member_stmts.push_str(&format!("\t{member_dest} = {expr}\n"));
                } else {
                    // The bound entry value wins when resolved; otherwise the
                    // member falls back to its own sources.
                    member_stmts.push_str(&format!(
                        "\tif {head_why} == \"\" {{\n\t\t{member_dest} = {expr}\n\t}} else {{\n{fallback}\t}}\n",
                        head_why = why_var(&head),
                        fallback = indent(
                            self.member_sources_stmts(member, &member_dest).trim_end_matches('\n')
                        ),
                    ));
                }
            } else {
                member_stmts.push_str(&self.member_sources_stmts(member, &member_dest));
            }
            block.push_str(&indent(member_stmts.trim_end_matches('\n')));
        }
        block.push_str(&format!("\t\t{dest} = composed\n\t}}\n"));
        self.push(&block);
    }

    /// A config member's own source chain (only `@env`/`@default` can appear
    /// inside a config).
    fn member_sources_stmts(&mut self, member: &EntryField, dest: &str) -> String {
        let stub = EntryField {
            name: member.name.clone(),
            target: member.target.clone(),
            sources: member.sources.clone(),
            format: None,
            transforms: vec![],
            select: None,
            binds: vec![],
            constraints: vec![],
            traits: vec![],
        };
        let guaranteed = stub.sources.iter().any(|s| matches!(s, Source::Default(_)));
        if guaranteed {
            self.chain_cascade(&stub, dest)
        } else {
            // No reason tracking inside a composition: an absent member simply
            // keeps its zero value (the entry-level requires cover consumed
            // chains).
            let mut out = String::new();
            for source in &stub.sources {
                if let Source::Env(name) = source {
                    let lookup = self.env_lookup(name);
                    let label = self.env_label(name);
                    out.push_str(&format!(
                        "\tif v, ok := {lookup}; ok && v != \"\" {{\n{parse}\t}}\n",
                        parse = indent(self.env_parse(&stub, dest, &label).trim_end_matches('\n')),
                    ));
                }
            }
            out
        }
    }
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
