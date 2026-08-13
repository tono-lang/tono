open Tono_frontend

(* Decode-error coverage for the FFI ext_lib JSON codec (Ir_json_extern). The
   happy path is already exercised by extern_test.ml's round-trip; every case
   below pins one "missing required key" or "wrong shape" branch, entered
   only through the exposed decode_ext_lib (the nested decoders are private
   to the module). *)

let tref = `Assoc [ ("prim", `String "string") ]
let lang_path = `Assoc [ ("lang", `String "go"); ("path", `String "p") ]
let foreign_field = `Assoc [ ("name", `String "f"); ("type", tref) ]

let foreign_struct =
  `Assoc [ ("name", `String "s"); ("fields", `List [ foreign_field ]) ]

let yields_pos = `Assoc [ ("name", `String "y"); ("type", tref) ]
let returns_value_field = `Assoc [ ("field", `List [ `String "a" ]) ]

let returns_field =
  `Assoc [ ("name", `String "r"); ("value", returns_value_field) ]

let returns_lit = `Assoc [ ("type", tref); ("fields", `List [ returns_field ]) ]
let error_binding = `Assoc [ ("sentinel", `String "S"); ("type", `String "T") ]

let extern_lang =
  `Assoc
    [
      ("lang", `String "go");
      ("symbol", `String "Sym");
      ("call_args", `List []);
      ("yields", `List [ yields_pos ]);
      ("returns", returns_lit);
      ("errors", `List [ error_binding ]);
    ]

let extern_param = `Assoc [ ("name", `String "p"); ("type", tref) ]

let extern_decl =
  `Assoc
    [
      ("name", `String "e");
      ("params", `List [ extern_param ]);
      ("return", tref);
      ("langs", `List [ extern_lang ]);
    ]

let opaque_type =
  `Assoc [ ("name", `String "t"); ("methods", `List [ extern_decl ]) ]

let ext_lib =
  `Assoc
    [
      ("name", `String "lib");
      ("langs", `List [ lang_path ]);
      ("structs", `List [ foreign_struct ]);
      ("types", `List [ opaque_type ]);
      ("externs", `List [ extern_decl ]);
    ]

(* [with_ k v j] returns [j] with key [k] rebound to [v] (an override, used to
   splice a broken nested piece into an otherwise-valid tree); [without k j]
   drops [k] entirely, to hit a "missing required key" branch. *)
let with_ k v = function
  | `Assoc kvs -> `Assoc ((k, v) :: List.remove_assoc k kvs)
  | j -> j

let without k = function
  | `Assoc kvs -> `Assoc (List.remove_assoc k kvs)
  | j -> j

let ok_decode name j =
  match Ir_json.decode_model j with
  | Ok _ -> Alcotest.failf "%s: expected an error" name
  | Error _ -> ()

(* Wraps a single ext_lib into a minimal, otherwise-valid module/model
   envelope so the private nested decoders are reachable only through the
   public Ir_json.decode_model entry point, matching how real IR is loaded. *)
let model_with lib : Ir.json =
  `Assoc
    [
      ( "modules",
        `List
          [
            `Assoc
              [
                ("name", `String "m");
                ("shapes", `List []);
                ("operations", `List []);
                ("extensions", `List []);
                ("ext_libs", `List [ lib ]);
                ("tests", `List []);
              ];
          ] );
      ("tono_ir_version", `Int Ir_json.current_ir_version);
    ]

let valid_round_trips () =
  match Ir_json.decode_model (model_with ext_lib) with
  | Ok _ -> ()
  | Error e -> Alcotest.failf "expected the valid fixture to decode: %s" e

let missing_ext_lib_name () =
  ok_decode "ext lib missing name" (model_with (without "name" ext_lib))

let missing_lang_path_lang () =
  let bad_lang_path = without "lang" lang_path in
  ok_decode "lang path missing lang"
    (model_with (with_ "langs" (`List [ bad_lang_path ]) ext_lib))

let missing_lang_path_path () =
  let bad_lang_path = without "path" lang_path in
  ok_decode "lang path missing path"
    (model_with (with_ "langs" (`List [ bad_lang_path ]) ext_lib))

let missing_foreign_struct_name () =
  let bad = without "name" foreign_struct in
  ok_decode "foreign struct missing name"
    (model_with (with_ "structs" (`List [ bad ]) ext_lib))

let missing_foreign_field_name () =
  let bad_field = without "name" foreign_field in
  let bad_struct = with_ "fields" (`List [ bad_field ]) foreign_struct in
  ok_decode "foreign field missing name"
    (model_with (with_ "structs" (`List [ bad_struct ]) ext_lib))

let missing_foreign_field_type () =
  let bad_field = without "type" foreign_field in
  let bad_struct = with_ "fields" (`List [ bad_field ]) foreign_struct in
  ok_decode "foreign field missing type"
    (model_with (with_ "structs" (`List [ bad_struct ]) ext_lib))

let with_extern_lang lang = with_ "langs" (`List [ lang ]) extern_decl
let with_extern lang = with_ "externs" (`List [ with_extern_lang lang ]) ext_lib

let missing_yields_name () =
  let bad = without "name" yields_pos in
  let lang = with_ "yields" (`List [ bad ]) extern_lang in
  ok_decode "yields position missing name" (model_with (with_extern lang))

let returns_value_bad_shape () =
  (* Neither 'field' nor 'select': the catch-all branch. *)
  let bad = `Assoc [ ("field", `List [ `String "a" ]); ("select", `Null) ] in
  let bad_returns_field = with_ "value" bad returns_field in
  let bad_returns = with_ "fields" (`List [ bad_returns_field ]) returns_lit in
  let lang = with_ "returns" bad_returns extern_lang in
  ok_decode "returns value bad shape" (model_with (with_extern lang))

let missing_returns_field_name () =
  let bad = without "name" returns_field in
  let bad_returns = with_ "fields" (`List [ bad ]) returns_lit in
  let lang = with_ "returns" bad_returns extern_lang in
  ok_decode "returns field missing name" (model_with (with_extern lang))

let missing_returns_field_value () =
  let bad = without "value" returns_field in
  let bad_returns = with_ "fields" (`List [ bad ]) returns_lit in
  let lang = with_ "returns" bad_returns extern_lang in
  ok_decode "returns field missing value" (model_with (with_extern lang))

let missing_returns_type () =
  let bad_returns = without "type" returns_lit in
  let lang = with_ "returns" bad_returns extern_lang in
  ok_decode "returns missing type" (model_with (with_extern lang))

let missing_error_binding_sentinel () =
  let bad = without "sentinel" error_binding in
  let lang = with_ "errors" (`List [ bad ]) extern_lang in
  ok_decode "error binding missing sentinel" (model_with (with_extern lang))

let missing_error_binding_type () =
  let bad = without "type" error_binding in
  let lang = with_ "errors" (`List [ bad ]) extern_lang in
  ok_decode "error binding missing type" (model_with (with_extern lang))

let missing_extern_lang_lang () =
  let bad = without "lang" extern_lang in
  ok_decode "extern lang missing lang" (model_with (with_extern_lang bad))

let missing_extern_lang_symbol () =
  let bad = without "symbol" extern_lang in
  ok_decode "extern lang missing symbol" (model_with (with_extern_lang bad))

let missing_extern_param_name () =
  let bad = without "name" extern_param in
  let bad_extern = with_ "params" (`List [ bad ]) extern_decl in
  ok_decode "extern param missing name"
    (model_with (with_ "externs" (`List [ bad_extern ]) ext_lib))

let missing_extern_param_type () =
  let bad = without "type" extern_param in
  let bad_extern = with_ "params" (`List [ bad ]) extern_decl in
  ok_decode "extern param missing type"
    (model_with (with_ "externs" (`List [ bad_extern ]) ext_lib))

let missing_extern_name () =
  let bad = without "name" extern_decl in
  ok_decode "extern missing name"
    (model_with (with_ "externs" (`List [ bad ]) ext_lib))

let missing_extern_return () =
  let bad = without "return" extern_decl in
  ok_decode "extern missing return"
    (model_with (with_ "externs" (`List [ bad ]) ext_lib))

let missing_opaque_type_name () =
  let bad = without "name" opaque_type in
  ok_decode "opaque type missing name"
    (model_with (with_ "types" (`List [ bad ]) ext_lib))

let () =
  Alcotest.run "extern-ir-decode"
    [
      ("sanity", [ Alcotest.test_case "valid decodes" `Quick valid_round_trips ]);
      ( "missing keys",
        [
          Alcotest.test_case "ext lib name" `Quick missing_ext_lib_name;
          Alcotest.test_case "lang path lang" `Quick missing_lang_path_lang;
          Alcotest.test_case "lang path path" `Quick missing_lang_path_path;
          Alcotest.test_case "foreign struct name" `Quick
            missing_foreign_struct_name;
          Alcotest.test_case "foreign field name" `Quick
            missing_foreign_field_name;
          Alcotest.test_case "foreign field type" `Quick
            missing_foreign_field_type;
          Alcotest.test_case "yields name" `Quick missing_yields_name;
          Alcotest.test_case "returns value bad shape" `Quick
            returns_value_bad_shape;
          Alcotest.test_case "returns field name" `Quick
            missing_returns_field_name;
          Alcotest.test_case "returns field value" `Quick
            missing_returns_field_value;
          Alcotest.test_case "returns type" `Quick missing_returns_type;
          Alcotest.test_case "error binding sentinel" `Quick
            missing_error_binding_sentinel;
          Alcotest.test_case "error binding type" `Quick
            missing_error_binding_type;
          Alcotest.test_case "extern lang lang" `Quick missing_extern_lang_lang;
          Alcotest.test_case "extern lang symbol" `Quick
            missing_extern_lang_symbol;
          Alcotest.test_case "extern param name" `Quick
            missing_extern_param_name;
          Alcotest.test_case "extern param type" `Quick
            missing_extern_param_type;
          Alcotest.test_case "extern name" `Quick missing_extern_name;
          Alcotest.test_case "extern return" `Quick missing_extern_return;
          Alcotest.test_case "opaque type name" `Quick missing_opaque_type_name;
        ] );
    ]
