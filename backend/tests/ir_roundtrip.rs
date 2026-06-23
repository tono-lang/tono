//! Backend half of the IR contract: every golden fixture must decode, and
//! re-encoding it must reproduce the same JSON data the frontend emitted.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use tono_backend::ir::{self, check_roundtrip, decode_model, Constraint, Prim, ShapeKind, Tref};

const FIXTURE_NAMES: [&str; 5] = [
    "list_charges",
    "nullable_charge",
    "open_enum_union",
    "primitives",
    "service_api",
];

const ALL_PRIMS: [&str; 16] = [
    "bool",
    "string",
    "bytes",
    "i8",
    "i16",
    "i32",
    "i64",
    "u8",
    "u16",
    "u32",
    "u64",
    "float",
    "timestamp",
    "date",
    "duration",
    "uuid",
];

fn fixtures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../ir-schema/fixtures")
}

fn read_fixture(name: &str) -> String {
    let path = fixtures_dir().join(format!("{name}.json"));
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("reading {path:?}: {e}"))
}

#[test]
fn fixtures_decode_and_reencode_as_data() {
    for name in FIXTURE_NAMES {
        let json = read_fixture(name);
        let rt = check_roundtrip(&json).unwrap_or_else(|e| panic!("decode {name}: {e}"));
        assert!(
            rt.equal,
            "fixture {name} did not round-trip:\n  original:  {}\n  reencoded: {}",
            rt.original, rt.reencoded
        );
    }
}

#[test]
fn all_primitives_roundtrip() {
    for s in ALL_PRIMS {
        let json = format!(r#"{{"prim":"{s}"}}"#);
        let t: Tref = serde_json::from_str(&json).unwrap();
        assert_eq!(serde_json::to_string(&t).unwrap(), json, "prim {s}");
    }
}

#[test]
fn rejects_decimal_primitive() {
    assert!(serde_json::from_str::<Prim>(r#""decimal""#).is_err());
    assert!(serde_json::from_str::<Tref>(r#"{"prim":"decimal"}"#).is_err());
}

#[test]
fn rejects_out_of_set_int_width() {
    assert!(serde_json::from_str::<Prim>(r#""i7""#).is_err());
    assert!(serde_json::from_str::<Prim>(r#""i128""#).is_err());
}

#[test]
fn rejects_malformed_tref() {
    // zero recognized variant keys
    assert!(serde_json::from_str::<Tref>(r#"{"nope":1}"#).is_err());
    // more than one recognized variant key
    assert!(serde_json::from_str::<Tref>(r#"{"prim":"i32","list":{"prim":"bool"}}"#).is_err());
    // ref missing its args
    assert!(serde_json::from_str::<Tref>(r#"{"ref":"x#Y"}"#).is_err());
    // stray sibling key
    assert!(serde_json::from_str::<Tref>(r#"{"prim":"i32","extra":1}"#).is_err());
    // map wrong arity
    assert!(serde_json::from_str::<Tref>(r#"{"map":[{"prim":"bool"}]}"#).is_err());
    // not an object
    assert!(serde_json::from_str::<Tref>(r#""bool""#).is_err());
}

#[test]
fn tref_uses_flat_ref_form() {
    let wire = r#"{"ref":"core#Page","args":[{"ref":"p#Charge","args":[]}]}"#;
    let t: Tref = serde_json::from_str(wire).unwrap();
    match &t {
        Tref::Ref { id, args } => {
            assert_eq!(id, "core#Page");
            assert_eq!(args.len(), 1);
        }
        _ => panic!("expected a ref"),
    }
    assert_eq!(serde_json::to_string(&t).unwrap(), wire);
}

#[test]
fn version_gate_rejects_unknown_version() {
    assert!(decode_model(r#"{"tono_ir_version":2,"modules":[]}"#).is_ok());
    assert!(decode_model(r#"{"tono_ir_version":3,"modules":[]}"#).is_err());
    assert!(decode_model(r#"{"tono_ir_version":1,"modules":[]}"#).is_err());
    assert!(decode_model(r#"{"tono_ir_version":0,"modules":[]}"#).is_err());
    assert!(decode_model(r#"{"modules":[]}"#).is_err());
}

// ── Number fidelity ─────────────────────────────────────────────────────

#[test]
fn float_text_divergence_is_tolerated() {
    // The frontend (yojson) and backend (serde_json) format small floats with
    // different text ("1e-05" vs "0.00001"), so the contract compares JSON data,
    // not bytes: the round-trip still holds.
    let doc = r#"{"tono_ir_version":2,"modules":[{"name":"m","operations":[],
        "shapes":[{"id":"s#S","kind":"structure","params":[],"traits":[],
        "members":[{"name":"a","target":{"prim":"float"},"required":true,
        "default":1e-05,"constraints":[{"multipleOf":1e-7}],"traits":[]}]}]}]}"#;
    let rt = check_roundtrip(doc).unwrap();
    assert!(
        rt.equal,
        "float text differences must not break the data round-trip"
    );
}

#[test]
fn large_integer_in_trait_value_is_exact() {
    // The "primitives" fixture carries a u64-range integer in a trait value.
    let json = read_fixture("primitives");
    assert!(json.contains("12345678901234567890"));
    let rt = check_roundtrip(&json).unwrap();
    assert!(rt.equal, "u64-range integer must round-trip exactly");
    assert!(
        rt.reencoded.to_string().contains("12345678901234567890"),
        "large integer must not be coerced"
    );
}

// ── Defaults: absent vs present-null ────────────────────────────────────

#[test]
fn present_null_default_survives_roundtrip() {
    let doc = r#"{"tono_ir_version":2,"modules":[{"name":"m","operations":[],
        "shapes":[{"id":"s#S","kind":"structure","params":[],"traits":[],
        "members":[{"name":"a","target":{"prim":"string"},"required":false,
        "default":null,"constraints":[],"traits":[]}]}]}]}"#;
    let rt = check_roundtrip(doc).unwrap();
    assert!(
        rt.equal,
        "present-null default must survive: {}",
        rt.reencoded
    );
    assert!(rt.reencoded.to_string().contains(r#""default":null"#));
}

#[test]
fn absent_default_stays_absent() {
    let doc = r#"{"tono_ir_version":2,"modules":[{"name":"m","operations":[],
        "shapes":[{"id":"s#S","kind":"structure","params":[],"traits":[],
        "members":[{"name":"a","target":{"prim":"string"},"required":true,
        "constraints":[],"traits":[]}]}]}]}"#;
    let rt = check_roundtrip(doc).unwrap();
    assert!(rt.equal);
    assert!(!rt.reencoded.to_string().contains("default"));
}

// ── Symmetric tolerance with the frontend decoder ───────────────────────

#[test]
fn decode_tolerates_omitted_optional_fields() {
    // The frontend decoder defaults these fields; the mirror must accept the
    // same partial documents instead of rejecting them.
    let doc = r#"{"tono_ir_version":2,"modules":[{"name":"m","shapes":[
        {"id":"s#S","kind":"structure"},
        {"id":"u#U","kind":"union","members":[]},
        {"id":"e#E","kind":"enum","backing":"string"},
        {"id":"v#V","kind":"service"},
        {"id":"o#O","kind":"operation"}
    ]}]}"#;
    let m = decode_model(doc).expect("partial document should decode");
    let shapes = &m.modules[0].shapes;
    match &shapes[1].kind {
        ShapeKind::Union { discriminator, .. } => assert_eq!(discriminator, "type"),
        _ => panic!("expected a union"),
    }
    match &shapes[2].kind {
        ShapeKind::Enum { .. } => (),
        _ => panic!("expected an enum"),
    }
    assert!(decode_model(r#"{"tono_ir_version":2}"#).is_ok());
}

#[test]
fn constraint_decode_tolerance_matches_frontend() {
    // exclMin/exclMax default to false when absent, as the frontend does.
    assert!(serde_json::from_str::<Constraint>(r#"{"range":{"min":1.0}}"#).is_ok());
    // an extra sibling key on the tagged wrapper is rejected on both sides.
    assert!(serde_json::from_str::<Constraint>(r#"{"pattern":"x","bogus":1}"#).is_err());
    // a wrong-typed exclusive flag is rejected on both sides.
    assert!(serde_json::from_str::<Constraint>(r#"{"range":{"exclMin":"yes"}}"#).is_err());
}

// ── Divergence sentinels ────────────────────────────────────────────────
// These guard the gate itself: they prove that a drifting mirror is actually
// caught, so the round-trip check cannot silently rot.

#[test]
fn sentinel_unmodeled_field_breaks_roundtrip() {
    // If a document grows a field the mirror does not model, the re-encode no
    // longer equals the original. This is exactly the signal the gate relies on.
    let json = read_fixture("nullable_charge");
    let mut v: serde_json::Value = serde_json::from_str(&json).unwrap();
    v.as_object_mut()
        .unwrap()
        .insert("surprise".into(), serde_json::json!(true));
    let mutated = serde_json::to_string(&v).unwrap();

    let rt = check_roundtrip(&mutated).unwrap(); // serde ignores unknown fields
    assert!(!rt.equal, "an unmodeled field must change the round-trip");
}

#[test]
fn no_mixin_construct() {
    // The IR has no mixin node: members are always inline. A stray "mixins" key
    // is unmodeled and therefore cannot survive a round-trip.
    let json = read_fixture("nullable_charge");
    let mut v: serde_json::Value = serde_json::from_str(&json).unwrap();
    v["modules"][0]["shapes"][0]["mixins"] = serde_json::json!(["base#Audited"]);
    let rt = check_roundtrip(&serde_json::to_string(&v).unwrap()).unwrap();
    assert!(
        !rt.equal,
        "a mixin key must not survive; the IR has no mixin node"
    );
}

#[test]
fn sentinel_missing_required_field_fails_decode() {
    // A member without its `required` field must be refused, not defaulted.
    let bad = r#"{"tono_ir_version":2,"modules":[{"name":"m","operations":[],
        "shapes":[{"id":"x#Y","kind":"structure","params":[],"traits":[],
        "members":[{"name":"a","target":{"prim":"bool"},"constraints":[],"traits":[]}]}]}]}"#;
    assert!(
        decode_model(bad).is_err(),
        "a member missing `required` must fail to decode"
    );
}

#[test]
fn crate_reports_a_version() {
    assert!(!tono_backend::version().is_empty());
}

#[test]
fn check_roundtrip_rejects_invalid_json() {
    assert!(ir::check_roundtrip("[").is_err());
    assert!(decode_model("{ not json").is_err());
}

// ── The live-pipe binary ────────────────────────────────────────────────
// Drives the same `ir_roundtrip` binary the cross-language gate uses, so its
// glue is exercised under test (and coverage), not only by the shell script.

fn run_mirror_bin(input: &str) -> std::process::Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_ir_roundtrip"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn ir_roundtrip");
    child
        .stdin
        .take()
        .unwrap()
        .write_all(input.as_bytes())
        .expect("write stdin");
    child.wait_with_output().expect("wait for ir_roundtrip")
}

#[test]
fn bin_accepts_a_matching_fixture() {
    let out = run_mirror_bin(&read_fixture("primitives"));
    assert!(out.status.success(), "a faithful round-trip must exit zero");
}

#[test]
fn bin_fails_on_bad_input() {
    let out = run_mirror_bin(r#"{"tono_ir_version":999,"modules":[]}"#);
    assert!(!out.status.success(), "wrong version must exit non-zero");
}

#[test]
fn bin_fails_on_mismatch() {
    // A document with an unmodeled field decodes but does not re-encode to the
    // same data, so the mirror reports a divergence and exits non-zero.
    let mut v: serde_json::Value = serde_json::from_str(&read_fixture("nullable_charge")).unwrap();
    v.as_object_mut()
        .unwrap()
        .insert("surprise".into(), serde_json::json!(true));
    let out = run_mirror_bin(&serde_json::to_string(&v).unwrap());
    assert!(!out.status.success(), "a divergence must exit non-zero");
    assert!(String::from_utf8_lossy(&out.stderr).contains("mismatch"));
}
