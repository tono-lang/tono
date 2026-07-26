//! The per-field resolution emitter: one field of the entry's construction
//! surface lowered to TypeScript statements, mirroring the Go emitter
//! statement for statement so both SDKs construct identically.

use super::checks::*;
use super::*;

pub(super) struct Resolver<'a, 'b> {
    pub(super) entry: &'a EntryModel<'a>,
    pub(super) module: &'a Module,
    pub(super) config: &'a CasingConfig,
    pub(super) helpers: &'b mut Helpers,
    pub(super) body: &'b mut String,
}

impl Resolver<'_, '_> {
    fn push(&mut self, s: &str) {
        self.body.push_str(s);
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

    fn guaranteed(&self, name: &str) -> bool {
        self.entry.field_guaranteed(name)
    }

    pub(super) fn emit_field(&mut self, field: &EntryField) {
        match self.entry.field_shape(field, self.module) {
            FieldShape::Config(shape) => self.emit_config(field, shape),
            FieldShape::Structured(shape) => self.emit_structured(field, shape),
            FieldShape::Json => self.emit_json(field),
            FieldShape::Scalar => self.emit_scalar(field),
        }
    }

    /// A scalar field, with the declared `@str::*` pipeline applied to the
    /// resolved value whatever idiom produced it (`@format` folds it into the
    /// template expression itself).
    fn emit_scalar(&mut self, field: &EntryField) {
        if field.format.is_some() {
            self.emit_format(field);
            return;
        }
        if field.sources.iter().any(|s| matches!(s, Source::Arg)) {
            let assign = format!(
                "    {} = {};\n",
                self.ident(&field.name),
                camel(&field.name)
            );
            self.push(&assign);
        } else if field.select.is_some() {
            self.emit_select(field);
        } else {
            self.emit_chain(field);
        }
        let dest = self.ident(&field.name);
        let transforms = self.transforms_stmt(field, &dest);
        self.push(&transforms);
    }

    /// The `@str::*` pipeline over an already-resolved destination. An
    /// unresolved chain holds the zero value, which every transform maps to
    /// itself, so the application is unconditional.
    fn transforms_stmt(&mut self, field: &EntryField, dest: &str) -> String {
        if field.transforms.is_empty() {
            return String::new();
        }
        let expr = as_template_string(dest, &field.target);
        let expr = apply_transforms(expr, &field.transforms, self.helpers);
        format!("    {dest} = {};\n", cast_string(&field.target, &expr))
    }

    fn emit_chain(&mut self, field: &EntryField) {
        let dest = self.ident(&field.name);
        if self.entry.is_guaranteed(field) {
            let stmts = self.chain_cascade(field, &dest);
            self.push(&stmts);
        } else {
            let why = why_var(&field.name);
            self.push(&format!("    let {why} = \"no source\";\n"));
            let stmts = self.chain_sequential(field, &dest, &why);
            self.push(&stmts);
        }
    }

    fn with_access(&self, field: &EntryField) -> String {
        format!("config.{}", field_camel(&field.name, self.config))
    }

    /// A guaranteed chain, spelled with a set-flag so every env variable is
    /// read exactly once and the `@default` closes the chain.
    fn chain_cascade(&mut self, field: &EntryField, dest: &str) -> String {
        // A chain that is just the default needs no flag.
        if let [Source::Default(v)] = field.sources.as_slice() {
            return format!("    {dest} = {};\n", literal(&field.target, v));
        }
        let flag = camel(&format!("{}_set", field.name));
        let mut out = format!("    let {flag} = false;\n");
        for source in &field.sources {
            match source {
                Source::With => {
                    let acc = self.with_access(field);
                    out.push_str(&format!(
                        "    if (!{flag} && {acc} !== undefined) {{\n      {dest} = {acc};\n      {flag} = true;\n    }}\n"
                    ));
                }
                Source::Env(name) => {
                    let lookup = self.env_lookup(name);
                    out.push_str(&format!(
                        "    if (!{flag}) {{\n      const v = {lookup};\n      if (v !== undefined) {{\n{parse}        {flag} = true;\n      }}\n    }}\n",
                        parse = self
                            .env_parse(field, dest, &self.env_label(name))
                            .lines()
                            .map(|l| format!("  {l}\n"))
                            .collect::<String>(),
                    ));
                }
                Source::Default(v) => {
                    out.push_str(&format!(
                        "    if (!{flag}) {{\n      {dest} = {};\n    }}\n",
                        literal(&field.target, v),
                    ));
                    return out;
                }
                Source::Arg => {}
            }
        }
        out
    }

    fn chain_sequential(&mut self, field: &EntryField, dest: &str, why: &str) -> String {
        let mut out = String::new();
        let mut first = true;
        for source in &field.sources {
            let step = match source {
                Source::With => {
                    let acc = self.with_access(field);
                    format!(
                        "if ({acc} !== undefined) {{\n      {dest} = {acc};\n      {why} = \"\";\n    }} else {{\n      {why} = \"not configured\";\n    }}\n"
                    )
                }
                Source::Env(name) => {
                    let lookup = self.env_lookup(name);
                    let label = self.env_label(name);
                    let miss = self.env_miss_reason(name);
                    let pre = self.env_name_prereq(name, why);
                    format!(
                        "{pre}{{\n      const v = {lookup};\n      if (v !== undefined) {{\n{parse}        {why} = \"\";\n      }} else {{\n        {why} = {miss};\n      }}\n    }}\n",
                        parse = self
                            .env_parse(field, dest, &label)
                            .lines()
                            .map(|l| format!("  {l}\n"))
                            .collect::<String>(),
                    )
                }
                // Both lines sit at the step's own depth (no internal
                // padding): the wrapper below owns the indentation, first or
                // nested.
                Source::Default(v) => format!(
                    "{dest} = {lit};\n{why} = \"\";\n",
                    lit = literal(&field.target, v),
                ),
                Source::Arg => continue,
            };
            let flat = matches!(source, Source::Default(_));
            if first {
                if flat {
                    out.push_str(
                        &step
                            .lines()
                            .map(|l| format!("    {l}\n"))
                            .collect::<String>(),
                    );
                } else {
                    out.push_str(&format!("    {step}"));
                }
                first = false;
            } else if flat {
                out.push_str(&format!(
                    "    if ({why} !== \"\") {{\n{body}    }}\n",
                    body = step
                        .lines()
                        .map(|l| format!("      {l}\n"))
                        .collect::<String>(),
                ));
            } else {
                out.push_str(&format!("    if ({why} !== \"\") {{\n    {step}    }}\n",));
            }
        }
        out
    }

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
            "if ({head_why} !== \"\") {{\n      {why} = \"{head} <- \" + {head_why};\n    }} else ",
            head_why = why_var(head),
        )
    }

    fn env_lookup(&mut self, name: &EnvName) -> String {
        self.helpers.read_env = true;
        match name {
            EnvName::Name(n) => format!("readEnv({n:?})"),
            EnvName::Field(fr) => {
                let expr = self.path_expr(&fr.field);
                let t = self.path_type(&fr.field);
                format!("readEnv({})", as_template_string(&expr, &t))
            }
        }
    }

    fn env_label(&self, name: &EnvName) -> String {
        match name {
            EnvName::Name(n) => format!("{n:?}"),
            EnvName::Field(fr) => {
                let t = self.path_type(&fr.field);
                as_template_string(&self.path_expr(&fr.field), &t)
            }
        }
    }

    fn env_miss_reason(&mut self, name: &EnvName) -> String {
        match name {
            EnvName::Name(n) => format!("{:?}", format!("env {n}: empty")),
            EnvName::Field(fr) => {
                let t = self.path_type(&fr.field);
                format!(
                    "\"env \" + {} + \": empty\"",
                    as_template_string(&self.path_expr(&fr.field), &t)
                )
            }
        }
    }

    /// Parse a raw env string `v` into the destination, by the declared type;
    /// a parse failure fails construction naming the variable and the type.
    fn env_parse(&mut self, field: &EntryField, dest: &str, label: &str) -> String {
        let t = &field.target;
        match t {
            Tref::Prim(Prim::Bool) => format!(
                "      if (v === \"true\" || v === \"1\") {{\n        {dest} = true;\n      }} else if (v === \"false\" || v === \"0\") {{\n        {dest} = false;\n      }} else {{\n        throw new Error(`${{{label}}}: invalid bool ${{JSON.stringify(v)}} (want true/false/1/0)`);\n      }}\n"
            ),
            Tref::Prim(
                p @ (Prim::I8 | Prim::I16 | Prim::I32 | Prim::U8 | Prim::U16 | Prim::U32),
            ) => {
                // Decimal digits only plus the type's own range, matching the
                // Go boundary (strconv with an explicit bit size, which takes
                // a sign only for the signed types).
                let (min, max) = int_bounds(p);
                let regex = if matches!(p, Prim::I8 | Prim::I16 | Prim::I32) {
                    "/^[+-]?[0-9]+$/"
                } else {
                    "/^[0-9]+$/"
                };
                format!(
                    "      {{\n        if (!{regex}.test(v)) {{\n          throw new Error(`${{{label}}}: invalid {prim} ${{JSON.stringify(v)}}`);\n        }}\n        const n = Number(v);\n        if (!Number.isInteger(n) || n < {min} || n > {max}) {{\n          throw new Error(`${{{label}}}: invalid {prim} ${{JSON.stringify(v)}}`);\n        }}\n        {dest} = n;\n      }}\n",
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
                    "      {{\n        if (!{regex}.test(v)) {{\n          throw new Error(`${{{label}}}: invalid {prim} ${{JSON.stringify(v)}}`);\n        }}\n        const n = BigInt(v.startsWith(\"+\") ? v.slice(1) : v);\n        if (n < {min} || n > {max}) {{\n          throw new Error(`${{{label}}}: invalid {prim} ${{JSON.stringify(v)}}`);\n        }}\n        {dest} = n;\n      }}\n",
                    prim = prim_name(p),
                )
            }
            Tref::Prim(Prim::Float) => format!(
                // Decimal notation only: bare Number() also accepts hex and
                // Infinity spellings the Go boundary rejects.
                "      {{\n        if (!/^[+-]?(\\d+(\\.\\d*)?|\\.\\d+)([eE][+-]?\\d+)?$/.test(v)) {{\n          throw new Error(`${{{label}}}: invalid float ${{JSON.stringify(v)}}`);\n        }}\n        const n = Number(v);\n        if (!Number.isFinite(n)) {{\n          throw new Error(`${{{label}}}: invalid float ${{JSON.stringify(v)}}`);\n        }}\n        {dest} = n;\n      }}\n"
            ),
            Tref::Prim(Prim::Bytes) => format!(
                // The env boundary carries bytes the same way the wire does:
                // base64 text.
                "      try {{\n        {dest} = decodeBytes(v);\n      }} catch {{\n        throw new Error(`${{{label}}}: invalid base64 ${{JSON.stringify(v)}}`);\n      }}\n"
            ),
            Tref::Prim(Prim::Duration) => {
                self.helpers.duration_ms = true;
                format!(
                    "      try {{\n        durationToMs(v);\n      }} catch {{\n        throw new Error(`${{{label}}}: invalid duration ${{JSON.stringify(v)}}`);\n      }}\n      {dest} = v as Duration;\n"
                )
            }
            _ => format!("      {dest} = {};\n", cast_string(t, "v")),
        }
    }

    fn emit_select(&mut self, field: &EntryField) {
        let Some(select) = field.select.clone() else {
            return;
        };
        let dest = self.ident(&field.name);
        let guaranteed = self.entry.is_guaranteed(field);
        let why = why_var(&field.name);
        if !guaranteed {
            self.push(&format!("    let {why} = \"\";\n"));
        }
        let subject_head = select.subject.first().cloned().unwrap_or_default();
        let subject_expr = self.path_expr(&select.subject);
        let mut arms = String::new();
        let mut saw_wildcard = false;
        for arm in &select.arms {
            let stmts = self.arm_stmts(field, &arm.value, &dest, &why, guaranteed);
            match &arm.pattern {
                Some(p) => arms.push_str(&format!(
                    "      case {}: {{\n{stmts}        break;\n      }}\n",
                    pattern_literal(p),
                )),
                None => {
                    saw_wildcard = true;
                    arms.push_str(&format!(
                        "      default: {{\n{stmts}        break;\n      }}\n"
                    ));
                }
            }
        }
        if !saw_wildcard {
            // Every enum is open, so an undeclared value can still arrive at
            // run time even with total declared coverage. Failing construction
            // beats freezing a silent zero value into the resolved settings.
            let miss = if guaranteed {
                format!(
                    "        throw new Error(`{field}: match on {subject}: unmatched value ${{String({subject_expr})}}`);\n",
                    field = field.name,
                    subject = subject_head,
                )
            } else {
                format!("        {why} = \"match: unmatched value\";\n")
            };
            arms.push_str(&format!(
                "      default: {{\n{miss}        break;\n      }}\n"
            ));
        }
        let switch = format!("    switch ({subject_expr}) {{\n{arms}    }}\n");
        if !self.guaranteed(&subject_head) {
            self.push(&format!(
                "    if ({subj_why} !== \"\") {{\n      {why} = \"{subject_head} <- \" + {subj_why};\n    }} else {{\n  {switch}    }}\n",
                subj_why = why_var(&subject_head),
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
            ArmValue::Lit(v) => format!("        {dest} = {};\n", literal(&field.target, v)),
            ArmValue::Field(path) => {
                let head = path.first().cloned().unwrap_or_default();
                let expr = self.path_expr(path);
                if self.guaranteed(&head) {
                    format!("        {dest} = {expr};\n")
                } else {
                    format!(
                        "        if ({head_why} !== \"\") {{\n          {why} = \"{head} <- \" + {head_why};\n        }} else {{\n          {dest} = {expr};\n        }}\n",
                        head_why = why_var(&head),
                    )
                }
            }
            ArmValue::Sources(sources) => {
                let stub = source_stub(field, sources.clone());
                let inner = if guaranteed {
                    self.chain_cascade(&stub, dest)
                } else {
                    format!(
                        "    {why} = \"no source\";\n{}",
                        self.chain_sequential(&stub, dest, why)
                    )
                };
                inner
                    .lines()
                    .map(|l| format!("    {l}\n"))
                    .collect::<String>()
            }
        }
    }

    fn emit_format(&mut self, field: &EntryField) {
        let Some(format_parts) = field.format.clone() else {
            return;
        };
        let dest = self.ident(&field.name);
        let (concat, absent_deps) = self.format_pieces(&format_parts);
        let expr = apply_transforms(concat.join(" + "), &field.transforms, self.helpers);
        let assign = format!("    {dest} = {};\n", cast_string(&field.target, &expr));
        if absent_deps.is_empty() {
            self.push(&assign);
            return;
        }
        let why = why_var(&field.name);
        let mut out = format!("    let {why} = \"\";\n");
        for (i, dep) in absent_deps.iter().enumerate() {
            out.push_str(&format!(
                "{}if ({dep_why} !== \"\") {{\n      {why} = \"{dep} <- \" + {dep_why};\n    }}",
                if i == 0 { "    " } else { " else " },
                dep_why = why_var(dep),
            ));
        }
        out.push_str(&format!(" else {{\n  {assign}    }}\n"));
        self.push(&out);
    }

    /// The template split for emission: each part as a TypeScript string
    /// expression, plus the non-guaranteed heads the template depends on (in
    /// first-appearance order).
    fn format_pieces(&mut self, parts: &[TemplatePart]) -> (Vec<String>, Vec<String>) {
        let mut concat: Vec<String> = Vec::new();
        let mut absent_deps: Vec<String> = Vec::new();
        for part in parts {
            match part {
                TemplatePart::Lit(s) => concat.push(format!("{s:?}")),
                TemplatePart::Field(p) => {
                    let head = p.first().cloned().unwrap_or_default();
                    if !self.guaranteed(&head) && !absent_deps.contains(&head) {
                        absent_deps.push(head.clone());
                    }
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

    /// A structured source: an explicit `@arg`/`@with` value passes typed, a
    /// JSON env value decodes strictly (required members first, then unknown
    /// fields, then per-member scalar type checks, mirroring the Go order and
    /// strictness), and declared validation runs at construction.
    /// The shared opening of a decoded field (structured or whole-JSON): an
    /// explicit `@arg` value passes typed, the why-var opens the chain
    /// (without `@arg` such a field is never guaranteed, `@default` does not
    /// apply to it), an optional `@with` layer wins over the env decode, and
    /// the env source (when there is one) yields its destination, why-var,
    /// lookup, label, and miss reason.
    fn decode_opening(
        &mut self,
        field: &EntryField,
    ) -> Option<(String, String, String, String, String)> {
        let dest = self.ident(&field.name);
        if field.sources.iter().any(|s| matches!(s, Source::Arg)) {
            let assign = format!("    {dest} = {};\n", camel(&field.name));
            self.push(&assign);
            return None;
        }
        let why = why_var(&field.name);
        self.push(&format!("    let {why} = \"no source\";\n"));
        if field.sources.iter().any(|s| matches!(s, Source::With)) {
            let acc = self.with_access(field);
            self.push(&format!(
                "    if ({acc} !== undefined) {{\n      {dest} = {acc};\n      {why} = \"\";\n    }} else {{\n      {why} = \"not configured\";\n    }}\n"
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

    fn emit_structured(&mut self, field: &EntryField, shape: &Shape) {
        let Some((dest, why, lookup, label, miss)) = self.decode_opening(field) else {
            return;
        };
        let ty = type_ident_from_id(&shape.id);
        let mut known = Vec::new();
        let mut required_checks = String::new();
        let mut type_checks = String::new();
        if let ShapeKind::Structure { members, .. } = &shape.kind {
            for m in members {
                known.push(format!("{:?}", m.name));
                if m.required {
                    // An explicit null is as absent as a missing key, the
                    // same rule the Go probe applies.
                    required_checks.push_str(&format!(
                        "      if (!({name:?} in parsed) || record[{name:?}] === null) {{\n        throw new Error(`${{{label}}}: missing field {name}`);\n      }}\n",
                        name = m.name,
                    ));
                }
                // Scalar wire-type checks keep the strictness on par with the
                // Go decoder (which is typed); containers and refs decode as
                // the wire codec always has.
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
                        "      if ({guard}{present}typeof record[{name:?}] !== {ts_typeof:?}) {{\n        throw new Error(`${{{label}}}: field {name} must be {describe}`);\n      }}\n",
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
                "      const vs = validate{ty}(decoded);\n      if (vs.length > 0) {{\n        throw new {validation}(vs);\n      }}\n",
                validation = en.validation,
            )
        } else {
            String::new()
        };
        let block = format!(
            "    if ({why} !== \"\") {{\n      const raw = {lookup};\n      if (raw !== undefined) {{\n\
             \x20       let parsed: unknown;\n\
             \x20       try {{\n          parsed = JSON.parse(raw);\n        }} catch (e) {{\n          throw new Error(`${{{label}}}: ${{String(e)}}`);\n        }}\n\
             \x20       if (typeof parsed !== \"object\" || parsed === null || Array.isArray(parsed)) {{\n          throw new Error(`${{{label}}}: expected an object`);\n        }}\n\
             \x20       const record = parsed as Record<string, unknown>;\n\
             {required}\
             \x20       for (const key of Object.keys(parsed)) {{\n          if (![{known}].includes(key)) {{\n            throw new Error(`${{{label}}}: unknown field ${{key}}`);\n          }}\n        }}\n\
             {types}\
             \x20       const decoded = decode{ty}(parsed);\n\
             {validate}\
             \x20       {dest} = decoded;\n\
             \x20       {why} = \"\";\n\
             \x20     }} else {{\n        {why} = {miss};\n      }}\n    }}\n",
            known = known.join(", "),
            required = indent2(&required_checks),
            types = indent2(&type_checks),
            validate = indent2(&validate),
        );
        self.push(&block);
    }

    /// A map/list field: an explicit `@arg`/`@with` value passes typed, an env
    /// value decodes as JSON whole.
    fn emit_json(&mut self, field: &EntryField) {
        let Some((dest, why, lookup, label, miss)) = self.decode_opening(field) else {
            return;
        };
        let ty = ts_type(&field.target);
        // The parsed JSON runs through the same wire decode the codecs use,
        // so an i64 map or a union field lands typed, not as raw JSON shapes.
        let decode =
            crate::codegen::targets::typescript::codecs::decode_expr("parsed", &field.target);
        // Container and element checks keep the boundary as strict as Go's
        // typed unmarshal: the same env value must construct in both targets.
        let checks = json_shape_checks(&field.target, &label);
        let block = format!(
            "    if ({why} !== \"\") {{\n      const raw = {lookup};\n      if (raw !== undefined) {{\n        let parsed: any;\n        try {{\n          parsed = JSON.parse(raw);\n        }} catch (e) {{\n          throw new Error(`${{{label}}}: ${{String(e)}}`);\n        }}\n{checks}        {dest} = {decode} as {ty};\n        {why} = \"\";\n      }} else {{\n        {why} = {miss};\n      }}\n    }}\n"
        );
        self.push(&block);
    }

    fn emit_config(&mut self, field: &EntryField, shape: &Shape) {
        let ShapeKind::Config { fields } = &shape.kind else {
            return;
        };
        let ty = type_ident_from_id(&shape.id);
        let dest = self.ident(&field.name);
        let mut block = format!("    {{\n      const composed = {{}} as {ty};\n");
        for member in fields {
            let member_dest = format!("composed.{}", field_camel(&member.name, self.config));
            let bind = field.binds.iter().find(|b| b.field == member.name);
            let mut member_stmts = String::new();
            if let Some(bind) = bind {
                let head = bind.source.first().cloned().unwrap_or_default();
                let expr = self.path_expr(&bind.source);
                if self.guaranteed(&head) {
                    member_stmts.push_str(&format!("    {member_dest} = {expr};\n"));
                } else {
                    member_stmts.push_str(&format!(
                        "    if ({head_why} === \"\") {{\n      {member_dest} = {expr};\n    }} else {{\n{fallback}    }}\n",
                        head_why = why_var(&head),
                        fallback = self
                            .member_sources_stmts(member, &member_dest)
                            .lines()
                            .map(|l| format!("  {l}\n"))
                            .collect::<String>(),
                    ));
                }
            } else {
                member_stmts.push_str(&self.member_sources_stmts(member, &member_dest));
            }
            block.push_str(
                &member_stmts
                    .lines()
                    .map(|l| format!("  {l}\n"))
                    .collect::<String>(),
            );
        }
        block.push_str(&format!("      {dest} = composed;\n    }}\n"));
        self.push(&block);
    }

    /// A config member's own resolution: a match, a `@format` derivation, or
    /// its source chain (`@env`/`@default`), plus its declared `@str::*`
    /// pipeline. There is no reason tracking inside a composition: an absent
    /// member keeps its zero value (the entry-level requires cover consumed
    /// chains), so absence guards read the sibling why-vars directly.
    fn member_sources_stmts(&mut self, member: &EntryField, dest: &str) -> String {
        let mut out = if member.select.is_some() {
            self.member_select_stmts(member, dest)
        } else if member.format.is_some() {
            self.member_format_stmts(member, dest)
        } else {
            self.member_chain(&source_stub(member, member.sources.clone()), dest)
        };
        if member.format.is_none() {
            out.push_str(&self.transforms_stmt(member, dest));
        }
        out
    }

    fn member_chain(&mut self, stub: &EntryField, dest: &str) -> String {
        let guaranteed = stub.sources.iter().any(|s| matches!(s, Source::Default(_)));
        if guaranteed {
            self.chain_cascade(stub, dest)
        } else {
            let mut out = String::new();
            for source in &stub.sources {
                if let Source::Env(name) = source {
                    let lookup = self.env_lookup(name);
                    let label = self.env_label(name);
                    out.push_str(&format!(
                        "    {{\n      const v = {lookup};\n      if (v !== undefined) {{\n{parse}      }}\n    }}\n",
                        parse = self.env_parse(stub, dest, &label),
                    ));
                }
            }
            out
        }
    }

    /// A member's match, lowered like the entry-level one but without a
    /// why-var: an absent subject or an unmatched value leaves the member's
    /// zero value.
    fn member_select_stmts(&mut self, member: &EntryField, dest: &str) -> String {
        let Some(select) = member.select.clone() else {
            return String::new();
        };
        let subject_head = select.subject.first().cloned().unwrap_or_default();
        let subject_expr = self.path_expr(&select.subject);
        let mut arms = String::new();
        for arm in &select.arms {
            let stmts = self.member_arm_stmts(member, &arm.value, dest);
            match &arm.pattern {
                Some(p) => arms.push_str(&format!(
                    "      case {}: {{\n{stmts}        break;\n      }}\n",
                    pattern_literal(p),
                )),
                None => arms.push_str(&format!(
                    "      default: {{\n{stmts}        break;\n      }}\n"
                )),
            }
        }
        let switch = format!("    switch ({subject_expr}) {{\n{arms}    }}\n");
        if self.guaranteed(&subject_head) {
            switch
        } else {
            format!(
                "    if ({subj_why} === \"\") {{\n{sw}    }}\n",
                subj_why = why_var(&subject_head),
                sw = switch
                    .lines()
                    .map(|l| format!("  {l}\n"))
                    .collect::<String>(),
            )
        }
    }

    fn member_arm_stmts(&mut self, member: &EntryField, value: &ArmValue, dest: &str) -> String {
        match value {
            ArmValue::Lit(v) => format!("        {dest} = {};\n", literal(&member.target, v)),
            ArmValue::Field(path) => {
                let head = path.first().cloned().unwrap_or_default();
                let expr = self.path_expr(path);
                if self.guaranteed(&head) {
                    format!("        {dest} = {expr};\n")
                } else {
                    format!(
                        "        if ({head_why} === \"\") {{\n          {dest} = {expr};\n        }}\n",
                        head_why = why_var(&head),
                    )
                }
            }
            ArmValue::Sources(sources) => {
                let inner = self.member_chain(&source_stub(member, sources.clone()), dest);
                inner
                    .lines()
                    .map(|l| format!("    {l}\n"))
                    .collect::<String>()
            }
        }
    }

    /// A member's `@format` derivation: assigned only once every
    /// non-guaranteed head it reads has resolved.
    fn member_format_stmts(&mut self, member: &EntryField, dest: &str) -> String {
        let Some(format_parts) = member.format.clone() else {
            return String::new();
        };
        let (concat, absent_deps) = self.format_pieces(&format_parts);
        let expr = apply_transforms(concat.join(" + "), &member.transforms, self.helpers);
        let assign = format!("    {dest} = {};\n", cast_string(&member.target, &expr));
        if absent_deps.is_empty() {
            return assign;
        }
        let cond = absent_deps
            .iter()
            .map(|dep| format!("{} === \"\"", why_var(dep)))
            .collect::<Vec<_>>()
            .join(" && ");
        format!(
            "    if ({cond}) {{\n{inner}    }}\n",
            inner = assign
                .lines()
                .map(|l| format!("  {l}\n"))
                .collect::<String>()
        )
    }
}
