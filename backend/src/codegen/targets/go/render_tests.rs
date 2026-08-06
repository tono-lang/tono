//! The Go renderer's tests, in a file of their own so the module stays inside
//! the source-size ceiling.

use super::*;
use crate::codegen::symbol::Symbol;
use crate::codegen::tree::{Alias, Function, Interface, Method, Raw, UnionDecl};

fn field(name: &str, ty: TypeExpr, nullable: bool, wire: &str) -> Field {
    Field {
        name: Symbol::builtin(name),
        ty,
        nullable,
        wire: Some(wire.to_string()),
        deprecated: None,
        doc: None,
    }
}

#[test]
fn imports_render_as_go_import_lines() {
    // Go imports the whole package, so the per-symbol names are ignored. Each
    // statement is one entry of the block, so it is the spelled path alone.
    assert_eq!(
        GoRules::default().render_import("billing::types", "encoding/json", &["json"]),
        "\"encoding/json\""
    );
    // A module path prefixes an SDK sub-package so the import resolves, but a
    // standard-library import is left verbatim.
    let rules = GoRules {
        go_module: Some("example.com/sdk".into()),
        current: "payments.charges::types".into(),
    };
    assert_eq!(
        rules.render_import(
            "payments.charges::types",
            "payments.common::types",
            &["Money"]
        ),
        "\"example.com/sdk/payments/common\""
    );
    // The SDK's shared group is a package of its own, under internal/.
    assert_eq!(
        rules.render_import("payments.charges::types", "::record", &["EncodeRecord"]),
        "\"example.com/sdk/internal/record\""
    );
    assert_eq!(
        rules.render_import("payments.charges::types", "encoding/json", &[]),
        "\"encoding/json\""
    );
    // An external package whose path segment is not a legal identifier (a
    // hyphenated module name) carries its alias explicitly, so the alias.X
    // references resolve without leaning on the package clause.
    assert_eq!(
        rules.render_import(
            "payments.charges::types",
            "github.com/example/some-package",
            &["alias"]
        ),
        "alias \"github.com/example/some-package\""
    );
    // The statements are one block, since gofmt will not fold loose ones.
    assert_eq!(
        rules.render_imports(vec!["\"fmt\"".into(), "\"os\"".into()]),
        "import (\n\t\"fmt\"\n\t\"os\"\n)"
    );
    // A single import needs no block, which is how Go is written by hand.
    assert_eq!(
        rules.render_imports(vec!["\"fmt\"".into()]),
        "import \"fmt\""
    );
}

#[test]
fn a_struct_renders_fields_with_json_tags() {
    let decl = Decl::Interface(Interface {
        name: Symbol::builtin("Charge"),
        params: vec![],
        fields: vec![
            field(
                "AccountID",
                TypeExpr::Ref(Symbol::builtin("int64")),
                false,
                "account_id",
            ),
            field(
                "Note",
                TypeExpr::Ref(Symbol::builtin("string")),
                true,
                "note",
            ),
            field("Tip", TypeExpr::Ref(Symbol::builtin("int64")), true, "tip"),
            field(
                "Secret",
                TypeExpr::Ref(Symbol::builtin("[]byte")),
                false,
                "secret",
            ),
        ],
        deprecated: None,
        doc: None,
    });
    let out = GoRules::default().render_decl(&decl);
    assert!(out.starts_with("type Charge struct {\n"));
    // The 64-bit integer is held natively but tagged `,string`.
    assert!(out.contains("\tAccountID int64 `json:\"account_id,string\"`\n"));
    // An optional scalar becomes a pointer with `,omitempty`.
    assert!(out.contains("\tNote *string `json:\"note,omitempty\"`\n"));
    // An optional 64-bit integer combines `,string` and `,omitempty`.
    assert!(out.contains("\tTip *int64 `json:\"tip,string,omitempty\"`\n"));
    // `bytes` is a plain tag; `encoding/json` base64-encodes a []byte natively.
    assert!(out.contains("\tSecret []byte `json:\"secret\"`\n"));
}

#[test]
fn a_generic_struct_renders_its_type_parameter_clause_with_the_any_bound() {
    // Go requires a constraint on each parameter; an unconstrained one is `any`.
    let decl = Decl::Interface(Interface {
        name: Symbol::builtin("Page"),
        params: vec!["T".into()],
        fields: vec![field(
            "Items",
            TypeExpr::list(TypeExpr::Ref(Symbol::builtin("T"))),
            false,
            "items",
        )],
        deprecated: None,
        doc: None,
    });
    assert_eq!(
        GoRules::default().render_decl(&decl),
        "type Page[T any] struct {\n\tItems []T `json:\"items\"`\n}"
    );
}

#[test]
fn collections_stay_slices_and_maps_even_when_optional() {
    let decl = Decl::Interface(Interface {
        name: Symbol::builtin("Bag"),
        params: vec![],
        fields: vec![
            field(
                "Tags",
                TypeExpr::list(TypeExpr::Ref(Symbol::builtin("string"))),
                true,
                "tags",
            ),
            field(
                "Meta",
                TypeExpr::map(
                    TypeExpr::Ref(Symbol::builtin("string")),
                    TypeExpr::Ref(Symbol::builtin("int32")),
                ),
                false,
                "meta",
            ),
        ],
        deprecated: None,
        doc: None,
    });
    let out = GoRules::default().render_decl(&decl);
    // An optional slice is not a pointer; it stays a slice, with no omitempty.
    assert!(out.contains("\tTags []string `json:\"tags\"`\n"));
    assert!(out.contains("\tMeta map[string]int32 `json:\"meta\"`\n"));
}

#[test]
fn a_field_without_a_wire_override_tags_with_its_name() {
    let decl = Decl::Interface(Interface {
        name: Symbol::builtin("Charge"),
        params: vec![],
        fields: vec![Field {
            name: Symbol::builtin("Id"),
            ty: TypeExpr::Ref(Symbol::builtin("string")),
            nullable: false,
            wire: None,
            deprecated: None,
            doc: None,
        }],
        deprecated: None,
        doc: None,
    });
    assert!(GoRules::default()
        .render_decl(&decl)
        .contains("\tId string `json:\"Id\"`\n"));
}

#[test]
fn an_enum_renders_a_named_string_and_its_constants() {
    let decl = Decl::Enum(EnumDecl {
        name: Symbol::builtin("Status"),
        members: vec![Symbol::builtin("pending"), Symbol::builtin("in_review")],
        member_docs: vec![None, None],
        backing: EnumRepr::String,
        deprecated: None,
        doc: None,
    });
    assert_eq!(
        GoRules::default().render_decl(&decl),
        "type Status string\n\nconst (\n\tStatusPending Status = \"pending\"\n\t\
         StatusInReview Status = \"in_review\"\n)"
    );
}

#[test]
fn an_int_backed_enum_renders_a_named_int_and_its_integer_constants() {
    let decl = Decl::Enum(EnumDecl {
        name: Symbol::builtin("HTTPCode"),
        members: vec![
            Symbol::builtin("ok"),
            Symbol::builtin("not_found"),
            Symbol::builtin("error"),
        ],
        member_docs: vec![None, None, None],
        backing: EnumRepr::Int(vec![200, 404, 500]),
        deprecated: None,
        doc: None,
    });
    assert_eq!(
        GoRules::default().render_decl(&decl),
        "type HTTPCode int\n\nconst (\n\tHTTPCodeOk HTTPCode = 200\n\t\
         HTTPCodeNotFound HTTPCode = 404\n\tHTTPCodeError HTTPCode = 500\n)"
    );
}

#[test]
fn an_empty_enum_is_just_the_named_string() {
    let decl = Decl::Enum(EnumDecl {
        name: Symbol::builtin("Empty"),
        members: vec![],
        member_docs: vec![],
        backing: EnumRepr::String,
        deprecated: None,
        doc: None,
    });
    assert_eq!(GoRules::default().render_decl(&decl), "type Empty string\n");
}

#[test]
fn a_deprecated_struct_field_and_enum_carry_godoc_comments() {
    let decl = Decl::Interface(Interface {
        name: Symbol::builtin("Charge"),
        params: vec![],
        fields: vec![Field {
            name: Symbol::builtin("Amount"),
            ty: TypeExpr::Ref(Symbol::builtin("uint64")),
            nullable: false,
            wire: Some("amount".into()),
            deprecated: Some("use AmountCents".into()),
            doc: None,
        }],
        deprecated: Some("use ChargeV2".into()),
        doc: None,
    });
    let out = GoRules::default().render_decl(&decl);
    // The godoc line sits directly above the type and the field, no blank line.
    assert!(out.contains("// Deprecated: use ChargeV2\ntype Charge struct"));
    assert!(out.contains("\t// Deprecated: use AmountCents\n\tAmount"));

    // A bare `@deprecated` (no reason) renders the plain marker above the enum.
    let enum_decl = Decl::Enum(EnumDecl {
        name: Symbol::builtin("Status"),
        members: vec![Symbol::builtin("open")],
        member_docs: vec![None],
        backing: EnumRepr::String,
        deprecated: Some(String::new()),
        doc: None,
    });
    assert!(GoRules::default()
        .render_decl(&enum_decl)
        .starts_with("// Deprecated:\ntype Status string"));
}

#[test]
fn doc_renders_as_flattened_godoc_on_the_struct_field_and_enum_member() {
    let decl = Decl::Interface(Interface {
        name: Symbol::builtin("Charge"),
        params: vec![],
        fields: vec![Field {
            name: Symbol::builtin("Amount"),
            ty: TypeExpr::Ref(Symbol::builtin("int64")),
            nullable: false,
            wire: Some("amount".into()),
            deprecated: None,
            doc: Some("The **amount** in minor units.".into()),
        }],
        deprecated: None,
        doc: Some("A billing charge.".into()),
    });
    let out = GoRules::default().render_decl(&decl);
    // godoc sits directly above the type and the field; Markdown is flattened.
    assert!(out.starts_with("// A billing charge.\ntype Charge struct"));
    assert!(out.contains("\t// The amount in minor units.\n\tAmount"));

    // A per-member doc renders above its const.
    let enum_decl = Decl::Enum(EnumDecl {
        name: Symbol::builtin("Status"),
        members: vec![Symbol::builtin("open"), Symbol::builtin("closed")],
        member_docs: vec![Some("Still open.".into()), None],
        backing: EnumRepr::String,
        deprecated: None,
        doc: None,
    });
    let out = GoRules::default().render_decl(&enum_decl);
    assert!(out.contains("\t// Still open.\n\tStatusOpen Status = \"open\""));
}

#[test]
fn type_expressions_render_idiomatically() {
    let rules = GoRules::default();
    assert_eq!(
        rules.render_type(&TypeExpr::list(TypeExpr::Ref(Symbol::builtin("Charge")))),
        "[]Charge"
    );
    assert_eq!(
        rules.render_type(&TypeExpr::map(
            TypeExpr::Ref(Symbol::builtin("string")),
            TypeExpr::Ref(Symbol::builtin("Charge")),
        )),
        "map[string]Charge"
    );
    assert_eq!(
        rules.render_type(&TypeExpr::nullable(TypeExpr::Ref(Symbol::builtin(
            "Charge"
        )))),
        "*Charge"
    );
    assert_eq!(
        rules.render_type(&TypeExpr::Generic(
            Symbol::builtin("Page"),
            vec![TypeExpr::Ref(Symbol::builtin("Charge"))],
        )),
        "Page[Charge]"
    );
    assert_eq!(
        rules.render_type(&TypeExpr::entries(
            TypeExpr::Ref(Symbol::builtin("int32")),
            TypeExpr::Ref(Symbol::builtin("string")),
        )),
        "Entries[int32, string]"
    );
}

#[test]
fn a_function_renders_with_its_signature_and_body() {
    let function = Decl::Function(Function {
        name: Symbol::builtin("Decode"),
        params: vec![field(
            "data",
            TypeExpr::Ref(Symbol::builtin("[]byte")),
            false,
            "data",
        )],
        ret: Some(TypeExpr::Ref(Symbol::builtin("error"))),
        body: FnBody::Raw {
            text: "\treturn nil".into(),
            refs: vec![],
        },
    });
    assert_eq!(
        GoRules::default().render_decl(&function),
        "func Decode(data []byte) error {\n\treturn nil\n}"
    );
}

#[test]
fn an_alias_renders_as_a_named_type() {
    let alias = Decl::Alias(Alias {
        name: Symbol::builtin("Uuid"),
        value: "string".into(),
    });
    assert_eq!(GoRules::default().render_decl(&alias), "type Uuid string");
}

#[test]
fn a_raw_item_renders_verbatim() {
    let raw = Decl::Raw(Raw {
        text: "func (m Method) foo() {}".into(),
        refs: vec![],
        ..Raw::default()
    });
    assert_eq!(
        GoRules::default().render_decl(&raw),
        "func (m Method) foo() {}"
    );
}

#[test]
fn union_and_method_arms_render_nothing_here() {
    let union = Decl::Union(UnionDecl {
        name: Symbol::builtin("Method"),
        discriminator: "type".into(),
        variants: vec![],
        deprecated: None,
        doc: None,
    });
    let method = Decl::Method(Method {
        name: Symbol::builtin("Ping"),
        params: vec![],
        ret: None,
        err: None,
        is_async: false,
        doc: None,
    });
    assert_eq!(GoRules::default().render_decl(&union), "");
    assert_eq!(GoRules::default().render_decl(&method), "");
}

#[test]
fn a_client_renders_a_blocking_interface_with_the_error_pair() {
    // Go lowers async and sync operations to the same blocking signature;
    // the error channel is the (T, error) pair, or a bare error with no
    // output.
    let decl = Decl::Client(crate::codegen::tree::ClientDecl {
        name: Symbol::builtin("Client"),
        methods: vec![
            Method {
                name: Symbol::builtin("CreateCharge"),
                params: vec![field(
                    "input",
                    TypeExpr::Ref(Symbol::builtin("CreateChargeInput")),
                    false,
                    "input",
                )],
                ret: Some(TypeExpr::Ref(Symbol::builtin("Charge"))),
                err: Some(TypeExpr::Ref(Symbol::builtin("error"))),
                is_async: true,
                doc: None,
            },
            Method {
                name: Symbol::builtin("Ping"),
                params: vec![],
                ret: None,
                err: Some(TypeExpr::Ref(Symbol::builtin("error"))),
                is_async: false,
                doc: None,
            },
        ],
    });
    assert_eq!(
        GoRules::default().render_decl(&decl),
        "type Client interface {\n\tCreateCharge(input CreateChargeInput) \
         (Charge, error)\n\tPing() error\n}"
    );
}
