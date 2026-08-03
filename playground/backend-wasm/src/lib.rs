//! Browser entry for the Rust codegen backend: decode IR JSON produced by the
//! OCaml frontend and emit the SDK files for one target, exactly like the CLI
//! preview pipeline (decode, layout check, generate). Output text is the
//! engine's rough layout: the official formatters are separate binaries the
//! browser cannot spawn, and the engine guarantees the rough text is valid.

use serde::Serialize;
use tono_backend::codegen::{casing_for, check_layout, generate_target, CodegenConfig, TargetKind};
use tono_backend::ir::decode_model;
use wasm_bindgen::prelude::*;

#[derive(Serialize, Debug)]
struct FileOut {
    path: String,
    text: String,
}

#[derive(Serialize, Debug)]
struct GenerateOut {
    files: Vec<FileOut>,
}

fn generate_files(ir_json: &str, target: &str) -> Result<GenerateOut, String> {
    let kind = TargetKind::parse(target).ok_or_else(|| format!("unknown target: {target}"))?;
    let model = decode_model(ir_json)?;
    // Go has no relative imports, so any multi-package output (an HTTP op
    // pulls in the shared runtime package) needs a module path. The CLI
    // preview does the same with its scaffold module name.
    let config = CodegenConfig {
        go_module: Some("tono_playground".to_string()),
        ..CodegenConfig::default()
    };
    check_layout(&model, &[kind], &config)?;
    let files = generate_target(&model, kind, &config, &casing_for(kind))?;
    Ok(GenerateOut {
        files: files
            .into_iter()
            .map(|f| FileOut {
                path: f.path.to_string_lossy().into_owned(),
                text: f.text,
            })
            .collect(),
    })
}

/// Returns `{"files": [{"path", "text"}, ...]}` as a JSON string, or throws a
/// JS string with the decode, layout, or generation error.
#[wasm_bindgen]
pub fn generate(ir_json: &str, target: &str) -> Result<String, JsValue> {
    let out = generate_files(ir_json, target).map_err(|e| JsValue::from_str(&e))?;
    serde_json::to_string(&out).map_err(|e| JsValue::from_str(&e.to_string()))
}

#[derive(Serialize, Debug)]
struct SymbolOut {
    id: String,
    ident: String,
    kind: &'static str,
}

fn lang_of(kind: TargetKind) -> &'static str {
    match kind {
        TargetKind::Rust => "rust",
        TargetKind::Go => "go",
        TargetKind::TypeScript => "typescript",
    }
}

fn symbol_index(ir_json: &str, target: &str) -> Result<Vec<SymbolOut>, String> {
    use tono_backend::codegen::{conventions, ops};
    use tono_backend::ir::ShapeKind;
    let kind = TargetKind::parse(target).ok_or_else(|| format!("unknown target: {target}"))?;
    let model = decode_model(ir_json)?;
    let lang = lang_of(kind);
    let casing = casing_for(kind);
    let mut out = Vec::new();
    for module in &model.modules {
        for shape in &module.shapes {
            let shape_kind = match &shape.kind {
                ShapeKind::Structure { .. } => "struct",
                ShapeKind::Union { .. } => "union",
                ShapeKind::Enum { .. } => "enum",
                ShapeKind::Service { .. } => "service",
                ShapeKind::Operation { .. } => "op",
                ShapeKind::Entry { .. } => "entry",
                ShapeKind::Config { .. } => "config",
            };
            out.push(SymbolOut {
                id: shape.id.clone(),
                ident: conventions::type_ident(shape, lang),
                kind: shape_kind,
            });
            if let ShapeKind::Entry { operations, .. } = &shape.kind {
                for op in operations {
                    out.push(SymbolOut {
                        id: op.id.clone(),
                        ident: ops::method_ident(op, &casing, lang),
                        kind: "op",
                    });
                }
            }
        }
        for op in &module.operations {
            out.push(SymbolOut {
                id: op.id.clone(),
                ident: ops::method_ident(op, &casing, lang),
                kind: "op",
            });
        }
    }
    Ok(out)
}

/// The in-code identifier every IR declaration becomes for a target, straight
/// from the codegen's own naming (casing plus @rename), so tooling never
/// re-derives names. Returns a JSON array of {id, ident, kind}.
#[wasm_bindgen]
pub fn symbols(ir_json: &str, target: &str) -> Result<String, JsValue> {
    let out = symbol_index(ir_json, target).map_err(|e| JsValue::from_str(&e))?;
    serde_json::to_string(&out).map_err(|e| JsValue::from_str(&e.to_string()))
}

#[wasm_bindgen]
pub fn ir_version() -> u32 {
    tono_backend::ir::TONO_IR_VERSION
}

#[cfg(test)]
mod tests {
    use super::generate_files;

    #[test]
    fn rejects_unknown_target() {
        let err = generate_files("{}", "cobol").unwrap_err();
        assert!(err.contains("unknown target"));
    }

    #[test]
    fn rejects_bad_ir() {
        let err = generate_files("not json", "ts").unwrap_err();
        assert!(!err.is_empty());
    }
}
