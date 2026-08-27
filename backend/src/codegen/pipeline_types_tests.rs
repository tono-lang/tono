//! The types-only emission a foreign-binding probe compiles against, in a
//! file of its own so the pipeline tests stay inside the source-size
//! ceiling.

use super::tests::union_model;
use super::*;
use crate::codegen::test_support::{member, structure};
use crate::ir::{Prim, Tref};

/// The types-only cut is what a foreign-binding probe compiles against: each
/// module's `types` file and the SDK-root files it may import, and nothing
/// that imports the foreign library (the ext glue, the entry, the codec) or
/// re-exports the surface (the barrel, the module tree, the manifest).
#[test]
fn generate_types_keeps_the_types_and_root_files_only() {
    let mut model = union_model();
    // A support-typed field pulls the root support file in, and an ext
    // library beside it is what the cut must leave out.
    model.modules[0].shapes[0] = structure(
        "payments#Account",
        vec![
            member(
                "method",
                Tref::Ref {
                    id: "payments#Method".into(),
                    args: vec![],
                },
                true,
            ),
            member("opened", Tref::Prim(Prim::Timestamp), true),
        ],
    );
    let config = CodegenConfig {
        go_module: Some("example.com/sdk".into()),
        ..CodegenConfig::default()
    };
    for (target, lang) in [(TargetKind::Go, "go"), (TargetKind::TypeScript, "ts")] {
        // The library bound in the one language generated here: an ext bound
        // in several languages is gated on a declared test covering it.
        let mut lib = crate::codegen::verify::fixtures::gearbox();
        lib.langs.retain(|l| l.lang == lang);
        for decl in lib
            .externs
            .iter_mut()
            .chain(lib.types.iter_mut().flat_map(|t| t.methods.iter_mut()))
        {
            decl.langs.retain(|l| l.lang == lang);
        }
        model.modules[0].ext_libs = vec![lib];
        let files = generate_types(&model, target, &config, &casing_for(target)).unwrap();
        let paths: Vec<String> = files.iter().map(|f| f.path.display().to_string()).collect();
        let whole = generate_target(&model, target, &config, &casing_for(target)).unwrap();
        assert!(whole.len() > files.len(), "{paths:?}");
        assert!(
            paths
                .iter()
                .any(|p| p.ends_with("payments/types.go") || p.ends_with("payments/types.ts")),
            "{paths:?}"
        );
        // The support type the field needs comes along, wherever the layout
        // put it (a single-module SDK folds it into the module's own file).
        assert!(
            files.iter().any(|f| f.text.contains("Timestamp")),
            "the support type the timestamp field needs: {paths:?}"
        );
        for p in &paths {
            assert!(
                !p.contains("_ext")
                    && !p.contains("codec")
                    && !p.ends_with("index.ts")
                    && !p.ends_with("package.json")
                    && !p.contains("client"),
                "{p} is not a types file"
            );
        }
        for f in &files {
            assert!(
                !f.text.contains("gearbox"),
                "{} names the library:\n{}",
                f.path.display(),
                f.text
            );
        }
        // The kept text is the same declaration the whole SDK carries.
        for f in &files {
            let same = whole
                .iter()
                .find(|w| w.path == f.path)
                .expect("kept file is a whole-SDK file");
            assert_eq!(same.text, f.text, "{}", f.path.display());
        }
    }
}
