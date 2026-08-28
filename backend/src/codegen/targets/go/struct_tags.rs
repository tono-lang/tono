//! The generation gate for the struct tags a wire struct's `go` block
//! declares. A declared tag is appended to the ones the emitter derives,
//! verbatim, never merged: a declared key the emitter derives itself
//! (`json`) would leave the field with two of them, and `encoding/json`
//! reads whichever comes first, so the binding is refused naming both tags.
//!
//! A tag is invisible to the compiler: a library reads it at run time, by
//! reflection, and a wrong or missing one fails there, not at build. The
//! gate stops the one defect the generator can see (the collision); a tag in
//! a form `reflect.StructTag` does not read passes through verbatim, for
//! the target's own vet to grade, since nothing about it is derived here.

use crate::codegen::entries::local_name;
use crate::codegen::output::TargetKind;
use crate::codegen::targets::go::render::json_tag;
use crate::codegen::targets::go::types::{field_of, go_casing, LANG};
use crate::ir::{ForeignLang, Model, ShapeKind};

/// The keys of a Go struct tag in its conventional form, `key:"value"`
/// pairs separated by spaces, read the way `reflect.StructTag.Lookup` reads
/// them: a key runs to the colon, a value is a quoted string with backslash
/// escapes. Reading stops at the first pair that breaks the form; what came
/// before still counts.
pub(crate) fn tag_keys(tag: &str) -> Vec<&str> {
    let mut keys = Vec::new();
    let mut rest = tag;
    loop {
        rest = rest.trim_start_matches(' ');
        let Some(colon) = rest.find(':') else { break };
        let key = &rest[..colon];
        if key.is_empty() || key.contains([' ', '"']) {
            break;
        }
        let mut value = rest[colon + 1..].chars();
        if value.next() != Some('"') {
            break;
        }
        let mut closed = false;
        let mut escaped = false;
        let mut consumed = colon + 2;
        for c in value {
            consumed += c.len_utf8();
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                closed = true;
                break;
            }
        }
        if !closed {
            break;
        }
        keys.push(key);
        rest = &rest[consumed..];
    }
    keys
}

/// Refuse a declared tag whose key the emitter derives for the same field,
/// naming both tags, when Go is among the targets generated (the block is
/// Go's; no other target reads it). A model with no tagged struct passes
/// untouched.
pub fn validate_struct_tags(model: &Model, targets: &[TargetKind]) -> Result<(), String> {
    if !targets.contains(&TargetKind::Go) {
        return Ok(());
    }
    let casing = go_casing();
    for module in &model.modules {
        for shape in &module.shapes {
            let ShapeKind::Structure { members, .. } = &shape.kind else {
                continue;
            };
            let tags = ForeignLang::struct_tags(shape, LANG);
            if tags.is_empty() {
                continue;
            }
            for member in members {
                let Some(declared) = tags.get(&member.name) else {
                    continue;
                };
                if !tag_keys(declared).contains(&"json") {
                    continue;
                }
                let derived = json_tag(&field_of(member, &casing, &tags));
                return Err(format!(
                    "module {}: struct {} field {}: the go block declares the tag `{declared}` \
                     and the generated tag is `json:\"{derived}\"`; a declared tag is appended \
                     to the generated ones, never merged, so drop its json key (the wire key \
                     is @wire's to change)",
                    module.name,
                    local_name(&shape.id),
                    member.name
                ));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::{Member, Module, Prim, Shape, Trait, Tref, TONO_IR_VERSION};

    #[test]
    fn keys_follow_the_reflect_form() {
        assert_eq!(tag_keys(r#"env:"HOST""#), vec!["env"]);
        assert_eq!(
            tag_keys(r#"env:"A B" default:"1.5" json:"x,omitempty""#),
            vec!["env", "default", "json"]
        );
        // An escaped quote stays inside the value.
        assert_eq!(tag_keys(r#"a:"x\"y" b:"z""#), vec!["a", "b"]);
        // Text off the form contributes no key, and stops the reading.
        assert_eq!(tag_keys("env=HOST"), Vec::<&str>::new());
        assert_eq!(tag_keys(r#"env:"HOST" json"#), vec!["env"]);
        assert_eq!(tag_keys(r#"env:"unterminated"#), Vec::<&str>::new());
        assert_eq!(tag_keys(""), Vec::<&str>::new());
    }

    fn member(name: &str, target: Tref) -> Member {
        Member {
            name: name.into(),
            target,
            required: true,
            default: None,
            constraints: vec![],
            traits: vec![],
        }
    }

    fn tagged_model(tags: &[(&str, &str)], head: Option<&str>) -> Model {
        let block = ForeignLang {
            lang: "go".into(),
            name: head.map(str::to_string),
            fields: tags
                .iter()
                .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
                .collect(),
        };
        Model {
            tono_ir_version: TONO_IR_VERSION,
            modules: vec![Module {
                name: "tuning".into(),
                shapes: vec![Shape {
                    id: "tuning#calibration".into(),
                    kind: ShapeKind::Structure {
                        params: vec![],
                        members: vec![
                            member("scale", Tref::Prim(Prim::Float)),
                            member("samples", Tref::Prim(Prim::I64)),
                        ],
                    },
                    traits: vec![Trait {
                        id: "foreign".into(),
                        value: serde_json::to_value(vec![block]).unwrap(),
                    }],
                }],
                operations: vec![],
                extensions: vec![],
                ext_libs: vec![],
                tests: vec![],
            }],
        }
    }

    #[test]
    fn a_tag_the_emitter_does_not_derive_passes() {
        let model = tagged_model(&[("scale", r#"env:"CALC_{profile}_SCALE""#)], None);
        assert_eq!(validate_struct_tags(&model, &[TargetKind::Go]), Ok(()));
    }

    #[test]
    fn a_json_key_is_refused_naming_both_tags() {
        let model = tagged_model(&[("samples", r#"json:"n" env:"N""#)], None);
        let err = validate_struct_tags(&model, &[TargetKind::Go]).unwrap_err();
        assert!(err.contains("struct calibration field samples"), "{err}");
        assert!(err.contains(r#"`json:"n" env:"N"`"#), "{err}");
        // The generated tag is the one the field would carry, options included.
        assert!(err.contains(r#"`json:"samples,string"`"#), "{err}");
    }

    #[test]
    fn the_gate_is_go_s() {
        let model = tagged_model(&[("samples", r#"json:"n""#)], None);
        assert_eq!(
            validate_struct_tags(&model, &[TargetKind::TypeScript, TargetKind::Rust]),
            Ok(())
        );
        assert!(validate_struct_tags(&model, &[TargetKind::TypeScript, TargetKind::Go]).is_err());
    }

    #[test]
    fn a_headed_block_declares_no_tag() {
        // An error struct's block: its keyed entries are field sources.
        let model = tagged_model(&[("samples", r#"json:"n""#)], Some("ErrCalibration"));
        assert_eq!(validate_struct_tags(&model, &[TargetKind::Go]), Ok(()));
    }

    #[test]
    fn generation_runs_the_gate() {
        let model = tagged_model(&[("scale", r#"json:"s""#)], None);
        let config = crate::codegen::CodegenConfig::default();
        let err = crate::codegen::generate(&model, &[TargetKind::Go], &config).unwrap_err();
        assert!(err.contains("never merged"), "{err}");
        assert!(crate::codegen::generate(&model, &[TargetKind::TypeScript], &config).is_ok());
    }
}
