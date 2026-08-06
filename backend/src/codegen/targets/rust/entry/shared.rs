//! The SDK-root groups an entry's resolution logic reaches: reading an
//! environment variable, parsing the duration spelling every target shares,
//! and the casing transforms an `@str::` pipeline lowers to. They serve every
//! entry of every module, so they ride the SDK's shared groups rather than a
//! copy per module (mirrors Go's `duration`/`casing` root groups and
//! TypeScript's `env`/`duration`/`casing` ones). A bytes-typed field needs no
//! group of its own: it reuses the `bytes` group's `base64_bytes::decode`,
//! the same helper a wire struct field routes through.
//!
//! Each declaration names itself (`Decl::raw_providing`), and every call site
//! records a real reference ([`super::shared_symbol`]/[`super::shared_slot`],
//! mirroring Go's own `apply_transforms`), so the assembler's pruning reaches
//! them exactly like any other declaration: an SDK that never transforms
//! casing ships none of [`casing_helpers`], down to the individual function
//! (one `@str::kebab` field pulls in `str_kebab` alone, not its siblings).

use crate::codegen::tree::Decl;

/// Reading an environment variable. Treats an unset and an empty variable the
/// same: empty means not set, per the declared-source contract every target
/// shares.
pub(super) fn env_helpers() -> Vec<Decl> {
    vec![Decl::raw_providing(
        "read_env",
        "pub fn read_env(name: &str) -> Option<String> {\n    \
         match std::env::var(name) {\n        \
         Ok(v) if !v.is_empty() => Some(v),\n        \
         _ => None,\n    \
         }\n}",
        Vec::new(),
    )]
}

/// Parsing the duration spelling the targets share (Go's `time.ParseDuration`
/// grammar, which TypeScript's `durationToMs` already mirrors) into
/// milliseconds: an optional sign, a bare `0`, or a run of `<number><unit>`
/// pairs (`ns`, `us`/`µs`/`μs`, `ms`, `s`, `m`, `h`); a syntactically invalid
/// remainder fails to parse.
pub(super) fn duration_helpers() -> Vec<Decl> {
    vec![Decl::raw_providing(
        "parse_duration_ms",
        "pub fn parse_duration_ms(v: &str) -> Result<f64, ()> {\n\
         \x20   let mut rest = v;\n\
         \x20   let mut sign = 1.0;\n\
         \x20   if let Some(r) = rest.strip_prefix('-') {\n\
         \x20       sign = -1.0;\n\
         \x20       rest = r;\n\
         \x20   } else if let Some(r) = rest.strip_prefix('+') {\n\
         \x20       rest = r;\n\
         \x20   }\n\
         \x20   if rest == \"0\" {\n\
         \x20       return Ok(0.0);\n\
         \x20   }\n\
         \x20   let units: [(&str, f64); 8] = [\n\
         \x20       (\"ns\", 1e-6),\n\
         \x20       (\"\\u{b5}s\", 1e-3),\n\
         \x20       (\"\\u{3bc}s\", 1e-3),\n\
         \x20       (\"us\", 1e-3),\n\
         \x20       (\"ms\", 1.0),\n\
         \x20       (\"s\", 1000.0),\n\
         \x20       (\"m\", 60000.0),\n\
         \x20       (\"h\", 3_600_000.0),\n\
         \x20   ];\n\
         \x20   let mut total = 0.0;\n\
         \x20   let mut consumed = 0usize;\n\
         \x20   let bytes = rest.as_bytes();\n\
         \x20   let mut i = 0usize;\n\
         \x20   while i < bytes.len() {\n\
         \x20       let start = i;\n\
         \x20       while i < bytes.len() && bytes[i].is_ascii_digit() {\n\
         \x20           i += 1;\n\
         \x20       }\n\
         \x20       let mut has_digits = i > start;\n\
         \x20       if i < bytes.len() && bytes[i] == b'.' {\n\
         \x20           i += 1;\n\
         \x20           let frac_start = i;\n\
         \x20           while i < bytes.len() && bytes[i].is_ascii_digit() {\n\
         \x20               i += 1;\n\
         \x20           }\n\
         \x20           has_digits = has_digits || i > frac_start;\n\
         \x20       }\n\
         \x20       if !has_digits {\n\
         \x20           break;\n\
         \x20       }\n\
         \x20       let num: f64 = match rest[start..i].parse() {\n\
         \x20           Ok(n) => n,\n\
         \x20           Err(_) => break,\n\
         \x20       };\n\
         \x20       let unit_start = i;\n\
         \x20       let matched = units.iter().find(|(u, _)| rest[unit_start..].starts_with(u));\n\
         \x20       let Some((u, mult)) = matched else {\n\
         \x20           break;\n\
         \x20       };\n\
         \x20       i = unit_start + u.len();\n\
         \x20       total += num * mult;\n\
         \x20       consumed = i;\n\
         \x20   }\n\
         \x20   if consumed != rest.len() || rest.is_empty() {\n\
         \x20       return Err(());\n\
         \x20   }\n\
         \x20   Ok(sign * total)\n\
         }",
        Vec::new(),
    )]
}

/// The casing transforms an `@str::` pipeline lowers to, named the way every
/// other Rust identifier here is (snake_case) — the shared plan's own
/// vocabulary is PascalCase (`StrUpperSnake`), which is translated at the
/// call site ([`super::apply_transforms`]), not spelled here.
pub(super) fn casing_helpers() -> Vec<Decl> {
    let mut decls = vec![Decl::raw_providing(
        "entry_transform_words",
        "fn entry_transform_words(s: &str) -> Vec<&str> {\n    \
         s.split(|c: char| c == ' ' || c == '-' || c == '_').filter(|w| !w.is_empty()).collect()\n\
         }",
        Vec::new(),
    )];
    for (name, body) in [
        (
            "str_upper_snake",
            "    entry_transform_words(&s).iter().map(|w| w.to_uppercase()).collect::<Vec<_>>().join(\"_\")",
        ),
        (
            "str_snake",
            "    entry_transform_words(&s).iter().map(|w| w.to_lowercase()).collect::<Vec<_>>().join(\"_\")",
        ),
        (
            "str_kebab",
            "    entry_transform_words(&s).iter().map(|w| w.to_lowercase()).collect::<Vec<_>>().join(\"-\")",
        ),
        (
            "str_pascal",
            "    entry_transform_words(&s)\n        .iter()\n        .map(|w| {\n            let mut c = w.chars();\n            match c.next() {\n                Some(f) => f.to_uppercase().collect::<String>() + &c.as_str().to_lowercase(),\n                None => String::new(),\n            }\n        })\n        .collect::<String>()",
        ),
    ] {
        // The shared plan's call sites splice the accumulator expression in
        // directly, which is sometimes a bare place (`s.team`) and sometimes
        // an owned temporary (a chained transform's result); taking `String`
        // by value covers both — a bare place read this way is always
        // immediately reassigned by the caller (`dest = str_snake(dest)`),
        // which Rust accepts (the move and the reassignment do not overlap).
        decls.push(Decl::raw_providing(
            name,
            format!("pub fn {name}(s: String) -> String {{\n{body}\n}}"),
            Vec::new(),
        ));
    }
    decls
}

/// The SDK-root groups this target emits, each named for what it holds.
/// Called whenever the model has any entry at all; each declaration's own
/// name is what lets the assembler prune it down to exactly what the model's
/// entries reference.
pub fn shared_groups() -> Vec<(&'static str, Vec<Decl>)> {
    vec![
        ("env", env_helpers()),
        ("duration", duration_helpers()),
        ("casing", casing_helpers()),
        ("http", super::transport_decls::internal_helpers()),
    ]
}
