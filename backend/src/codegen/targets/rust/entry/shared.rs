//! The SDK-root groups an entry's resolution logic reaches: reading an
//! environment variable, parsing the duration spelling every target shares,
//! and the casing transforms an `@str::` pipeline lowers to. They serve every
//! entry of every module, so they ride the SDK's shared groups rather than a
//! copy per module (mirrors Go's `duration`/`casing` root groups and
//! TypeScript's `env`/`duration`/`casing` ones). A bytes-typed field needs no
//! group of its own: it reuses the `bytes` group's `base64_bytes::decode`,
//! the same helper a wire struct field routes through.
//!
//! Declared with no `provides` list (matching this target's own
//! `number`/`bytes` helper modules): the assembler's per-declaration pruning
//! only trims a declaration it can name-check against a real reference, and
//! this crate builds an entry's `use` of these unconditionally on the
//! `Helpers` flags its own resolution logic set ([`super::surface::
//! helper_imports`]) rather than through a `Symbol` ref threaded to every
//! call site — so an unnamed declaration here is the one that survives.
//! Whichever of these groups a model reaches ships whole, mirroring Go's own
//! `duration`/`casing` root groups (gated only on "does the model have any
//! entry", not on which specific helper a given entry calls).

use crate::codegen::tree::Decl;

/// Reading an environment variable. Treats an unset and an empty variable the
/// same: empty means not set, per the declared-source contract every target
/// shares.
pub(super) fn env_helpers() -> Vec<Decl> {
    vec![Decl::raw(
        "pub fn read_env(name: &str) -> Option<String> {\n    \
         match std::env::var(name) {\n        \
         Ok(v) if !v.is_empty() => Some(v),\n        \
         _ => None,\n    \
         }\n}",
    )]
}

/// Parsing the duration spelling the targets share (Go's `time.ParseDuration`
/// grammar, which TypeScript's `durationToMs` already mirrors) into
/// milliseconds: an optional sign, a bare `0`, or a run of `<number><unit>`
/// pairs (`ns`, `us`/`µs`/`μs`, `ms`, `s`, `m`, `h`); a syntactically invalid
/// remainder fails to parse.
pub(super) fn duration_helpers() -> Vec<Decl> {
    vec![Decl::raw(
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
    )]
}

/// The casing transforms an `@str::` pipeline lowers to. The call sites are
/// spelled by the shared plan (`codegen::entries::plan::casing_transform`),
/// identically across every target (`strUpperSnake`/`strSnake`/`strKebab`/
/// `strPascal`), so the definitions here match that spelling exactly rather
/// than this crate's own snake_case convention — the one place a generated
/// Rust identifier is not snake_case, and it is a deliberate cross-target
/// contract, not an oversight.
pub(super) fn casing_helpers() -> Vec<Decl> {
    let mut decls = vec![Decl::raw(
        "pub fn entry_transform_words(s: &str) -> Vec<&str> {\n    \
         s.split(|c: char| c == ' ' || c == '-' || c == '_').filter(|w| !w.is_empty()).collect()\n\
         }",
    )];
    for (name, body) in [
        (
            "strUpperSnake",
            "    entry_transform_words(&s).iter().map(|w| w.to_uppercase()).collect::<Vec<_>>().join(\"_\")",
        ),
        (
            "strSnake",
            "    entry_transform_words(&s).iter().map(|w| w.to_lowercase()).collect::<Vec<_>>().join(\"_\")",
        ),
        (
            "strKebab",
            "    entry_transform_words(&s).iter().map(|w| w.to_lowercase()).collect::<Vec<_>>().join(\"-\")",
        ),
        (
            "strPascal",
            "    entry_transform_words(&s)\n        .iter()\n        .map(|w| {\n            let mut c = w.chars();\n            match c.next() {\n                Some(f) => f.to_uppercase().collect::<String>() + &c.as_str().to_lowercase(),\n                None => String::new(),\n            }\n        })\n        .collect::<String>()",
        ),
    ] {
        // The shared plan's call sites splice the accumulator expression in
        // directly, which is sometimes a bare place (`s.team`) and sometimes
        // an owned temporary (a chained transform's result); taking `String`
        // by value covers both — a bare place read this way is always
        // immediately reassigned by the caller (`dest = strSnake(dest)`),
        // which Rust accepts (the move and the reassignment do not overlap).
        decls.push(Decl::raw(format!(
            "#[allow(non_snake_case)]\npub fn {name}(s: String) -> String {{\n{body}\n}}"
        )));
    }
    decls
}

/// The SDK-root groups this target emits, each named for what it holds.
/// Called whenever the model has any entry at all.
pub fn shared_groups() -> Vec<(&'static str, Vec<Decl>)> {
    vec![
        ("env", env_helpers()),
        ("duration", duration_helpers()),
        ("casing", casing_helpers()),
    ]
}
