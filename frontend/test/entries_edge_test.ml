open Tono_frontend

(* Edge and error paths of the entry model: parser recovery, lowering
   diagnostics, wire-decoding rejections, typecheck rejections beyond the happy
   path, and the protocol descriptor's value-expression forms. *)

let parse_diags src = snd (Parser.parse src)
let parse_errors src = List.length (parse_diags src) > 0

let check src =
  let file, _ = Parser.parse src in
  let diags = ref [] in
  let m = Lower.lower_file ~module_name:"m" ~diags file in
  let _, tc = Typecheck.check_module ~file m in
  tc

let codes src = List.filter_map (fun (d : Diagnostic.t) -> d.code) (check src)

let lower_msgs src =
  let file, _ = Parser.parse src in
  let diags = ref [] in
  ignore (Lower.lower_file ~module_name:"m" ~diags file);
  List.map (fun (d : Diagnostic.t) -> d.message) !diags

let has_error what src = Alcotest.(check bool) what true (parse_errors src)
let wire = "struct r { y: string }\n"

let entry fields =
  "pub struct c {\n" ^ fields
  ^ "\n\
    \  ep: string @env(\"EP\")\n\
    \  op o(): r @http(method: \"GET\", path: \"/\", endpoint: .ep)\n\
     }\n" ^ wire

(* ── Parser error recovery ─────────────────────────────────────────────── *)

let eq_without_match () =
  has_error "= without match" "struct s { e: string = 5 }"

let match_subject_not_ref () =
  has_error "subject must be a ref" "struct s { e: string = match v { } }"

let match_missing_fat_arrow () =
  has_error "missing =>" "struct s { e: string = match .v { \"a\" .b } }"

let match_bad_pattern () =
  has_error "bad pattern" "struct s { e: string = match .v { ( => .b } }"

let match_bad_value () =
  has_error "bad arm value" "struct s { e: string = match .v { \"a\" => ( } }"

let match_unterminated () =
  has_error "unterminated match" "struct s { e: string = match .v { \"a\" => .b"

let catalog_missing_name () =
  has_error "missing catalog entry" "struct s { a: string @str:: }"

let ref_missing_name () =
  has_error "dot without name" "struct s { a: string @env(.) }"

let struct_body_junk () = has_error "junk in struct body" "struct s { => :: }"

let ctor_field_missing_name () =
  has_error "ctor field missing a name" "struct s { a: string @x(y { : .a }) }"

let ctor_missing_closing_brace () =
  has_error "ctor missing closing brace" "struct s { a: string @x(y { z: .a ) }"

let ctor_empty_fields_parses () =
  Alcotest.(check bool)
    "an empty ctor body parses" false
    (parse_errors "struct s { a: string @x(y {}) }")

let kv_ref_value_parses () =
  Alcotest.(check bool)
    "k: .a.b parses as a kv ref" false
    (parse_errors "struct s { a: string @x(k: .a.b) }")

(* ── fmt round-trips for the remaining print forms ─────────────────────── *)

let fmt_roundtrip src =
  let file, pd = Parser.parse src in
  Alcotest.(check int) "no parse diagnostics" 0 (List.length pd);
  let printed = Printer.print_file file in
  let reparsed, pd2 = Parser.parse printed in
  Alcotest.(check int) "printed form re-parses" 0 (List.length pd2);
  Alcotest.(check string)
    "printing is a fixpoint" printed
    (Printer.print_file reparsed)

let fmt_bool_and_int_matches () =
  fmt_roundtrip
    "struct s {\n\
    \  b: bool @env(\"B\")\n\
    \  i: i32 @env(\"I\")\n\
    \  x: string = match .b {\n\
    \    true => \"a\"\n\
    \    false => other\n\
    \  }\n\
    \  y: i32 = match .i {\n\
    \    1 => 10\n\
    \    _ => .i\n\
    \  }\n\
     }\n"

let fmt_sources_arm_and_ops_only () =
  fmt_roundtrip
    "struct s {\n\
    \  v: string @env(\"V\")\n\
    \  x: string = match .v {\n\
    \    _ => @env(\"A\") @default(\"b\")\n\
    \  }\n\
     }\n\n\
     struct t {\n\
    \  op ping()\n\
     }\n"

(* ── Lowering diagnostics ──────────────────────────────────────────────── *)

let contains hay needle =
  let hn = String.length hay and nn = String.length needle in
  let rec loop i =
    if i + nn > hn then false
    else if String.sub hay i nn = needle then true
    else loop (i + 1)
  in
  nn = 0 || loop 0

let lower_msg_contains what src fragment =
  Alcotest.(check bool)
    what true
    (List.exists (fun m -> contains m fragment) (lower_msgs src))

let env_bad_args () =
  lower_msg_contains "env with no args" "struct s { a: string @env, op ping() }"
    "@env expects";
  lower_msg_contains "env with int" "struct s { a: string @env(5), op ping() }"
    "@env expects"

let format_bad_args () =
  lower_msg_contains "format with int"
    "struct s { a: string @format(5) @env(\"A\"), op ping() }" "@format expects"

let bind_bad_args () =
  lower_msg_contains "bind with one arg"
    "struct s { a: string @env(\"A\") @bind(x), op ping() }" "@bind expects"

let template_unterminated () =
  lower_msg_contains "unterminated placeholder"
    "struct s { a: string @format(\"x{y\"), op ping() }" "unterminated '{'"

let arm_with_non_source_trait () =
  lower_msg_contains "non-source in arm"
    "struct s { v: string @env(\"V\")\n\
    \  a: string = match .v { _ => @doc(\"no\") }\n\
    \  op ping() }"
    "only stacks value sources"

(* ── Wire decoding rejections ──────────────────────────────────────────── *)

let ok_decode what f j =
  match f j with
  | Ok _ -> ()
  | Error e -> Alcotest.failf "%s: expected success, got %s" what e

let bad_decode what f j =
  match f j with
  | Ok _ -> Alcotest.failf "%s: expected an error" what
  | Error _ -> ()

let decode_sources () =
  ok_decode "arg" Ir_json.decode_source (`String "arg");
  ok_decode "with" Ir_json.decode_source (`String "with");
  ok_decode "env name" Ir_json.decode_source (`Assoc [ ("env", `String "N") ]);
  ok_decode "env field" Ir_json.decode_source
    (`Assoc [ ("env", `Assoc [ ("field", `List [ `String "a" ]) ]) ]);
  ok_decode "default null" Ir_json.decode_source (`Assoc [ ("default", `Null) ]);
  bad_decode "unknown string" Ir_json.decode_source (`String "nope");
  bad_decode "env int" Ir_json.decode_source (`Assoc [ ("env", `Int 1) ]);
  bad_decode "not a source" Ir_json.decode_source (`Int 1);
  bad_decode "two keys" Ir_json.decode_source
    (`Assoc [ ("env", `String "N"); ("default", `Null) ])

let decode_template_parts () =
  ok_decode "lit" Ir_json.decode_template_part (`Assoc [ ("lit", `String "x") ]);
  ok_decode "field" Ir_json.decode_template_part
    (`Assoc [ ("field", `List [ `String "a" ]) ]);
  ok_decode "input" Ir_json.decode_template_part
    (`Assoc [ ("input", `String "id") ]);
  bad_decode "lit int" Ir_json.decode_template_part (`Assoc [ ("lit", `Int 1) ]);
  bad_decode "unknown key" Ir_json.decode_template_part
    (`Assoc [ ("x", `Int 1) ])

let decode_arm_values_and_select () =
  ok_decode "lit null arm" Ir_json.decode_select
    (`Assoc
       [
         ("subject", `List [ `String "v" ]);
         ( "arms",
           `List
             [
               `Assoc
                 [
                   ("pattern", `String "a"); ("value", `Assoc [ ("lit", `Null) ]);
                 ];
               `Assoc
                 [ ("value", `Assoc [ ("sources", `List [ `String "arg" ]) ]) ];
             ] );
       ]);
  bad_decode "select without subject" Ir_json.decode_select
    (`Assoc [ ("arms", `List []) ]);
  bad_decode "arm without value" Ir_json.decode_select
    (`Assoc
       [
         ("subject", `List [ `String "v" ]);
         ("arms", `List [ `Assoc [ ("pattern", `String "a") ] ]);
       ]);
  bad_decode "arm with stray key" Ir_json.decode_select
    (`Assoc
       [
         ("subject", `List [ `String "v" ]);
         ( "arms",
           `List
             [
               `Assoc
                 [ ("value", `Assoc [ ("lit", `Null) ]); ("extra", `Int 1) ];
             ] );
       ]);
  bad_decode "empty arm value" Ir_json.decode_arm_value (`Assoc []);
  bad_decode "unknown arm value" Ir_json.decode_arm_value
    (`Assoc [ ("x", `Int 1) ])

let decode_binds_and_fields () =
  ok_decode "bind" Ir_json.decode_bind
    (`Assoc [ ("field", `String "a"); ("source", `List [ `String "b" ]) ]);
  bad_decode "bind without field" Ir_json.decode_bind
    (`Assoc [ ("source", `List []) ]);
  bad_decode "bind without source" Ir_json.decode_bind
    (`Assoc [ ("field", `String "a") ]);
  ok_decode "minimal entry field" Ir_json.decode_entry_field
    (`Assoc
       [
         ("name", `String "a"); ("target", `Assoc [ ("prim", `String "string") ]);
       ]);
  bad_decode "field without name" Ir_json.decode_entry_field
    (`Assoc [ ("target", `Assoc [ ("prim", `String "string") ]) ]);
  bad_decode "field without target" Ir_json.decode_entry_field
    (`Assoc [ ("name", `String "a") ]);
  ok_decode "minimal entry shape" Ir_json.decode_shape
    (`Assoc [ ("id", `String "m#e"); ("kind", `String "entry") ]);
  ok_decode "minimal config shape" Ir_json.decode_shape
    (`Assoc [ ("id", `String "m#c"); ("kind", `String "config") ])

(* ── Typecheck: match shapes across subject types ──────────────────────── *)

let expect what wanted src =
  Alcotest.(check (list string)) what wanted (codes src)

let bool_match_exhaustive_without_wildcard () =
  expect "bool covered" []
    (entry
       "  b: bool @env(\"B\")\n\
       \  x: string = match .b { true => \"a\" false => \"b\" }")

let bool_match_bad_pattern () =
  expect "string pattern on bool" [ "TC0041"; "TC0040" ]
    (entry "  b: bool @env(\"B\")\n  x: string = match .b { \"t\" => \"a\" }")

let int_match_needs_wildcard () =
  expect "int without wildcard" [ "TC0041" ]
    (entry "  i: i32 @env(\"I\")\n  x: i32 = match .i { 1 => 2 }")

let int_match_bad_pattern () =
  expect "name pattern on int" [ "TC0041"; "TC0040" ]
    (entry "  i: i32 @env(\"I\")\n  x: i32 = match .i { one => 2 }")

let string_match_bad_pattern () =
  expect "int pattern on string" [ "TC0040" ]
    (entry
       "  v: string @env(\"V\")\n\
       \  x: string = match .v { 1 => \"a\" _ => \"b\" }")

let enum_match_full_coverage () =
  expect "enum covered" []
    ("enum lvl { low, high }\n"
    ^ entry
        "  l: lvl @env(\"L\")\n\
        \  x: string = match .l { low => \"a\" high => \"b\" }")

let enum_match_unknown_case () =
  expect "unknown case" [ "TC0040" ]
    ("enum lvl { low, high }\n"
    ^ entry
        "  l: lvl @env(\"L\")\n\
        \  x: string = match .l { low => \"a\" nope => \"b\" _ => \"c\" }")

let enum_match_quoted_pattern () =
  expect "quoted enum pattern" [ "TC0041"; "TC0040" ]
    ("enum lvl { low, high }\n"
    ^ entry "  l: lvl @env(\"L\")\n  x: string = match .l { \"low\" => \"a\" }"
    )

let subject_not_scalar () =
  expect "duration subject" [ "TC0040" ]
    (entry "  d: duration @env(\"D\")\n  x: string = match .d { _ => \"a\" }")

let subject_unknown () =
  expect "unknown subject" [ "TC0038" ]
    (entry "  x: string = match .nope { _ => \"a\" }")

let duplicate_pattern () =
  expect "duplicate" [ "TC0040" ]
    (entry
       "  v: string @env(\"V\")\n\
       \  x: string = match .v { \"a\" => \"1\" \"a\" => \"2\" _ => \"3\" }")

let arm_after_wildcard () =
  expect "unreachable arm" [ "TC0040" ]
    (entry
       "  v: string @env(\"V\")\n\
       \  x: string = match .v { _ => \"1\" \"a\" => \"2\" }")

let arm_ref_unknown () =
  expect "unknown arm ref" [ "TC0038" ]
    (entry "  v: string @env(\"V\")\n  x: string = match .v { _ => .nope }")

let arm_source_ref_unknown () =
  expect "unknown arm source ref" [ "TC0038" ]
    (entry
       "  v: string @env(\"V\")\n  x: string = match .v { _ => @env(.nope) }")

let arm_stacks_only_sources () =
  expect "doc in arm" [ "TC0040" ]
    (entry
       "  v: string @env(\"V\")\n  x: string = match .v { _ => @doc(\"no\") }")

(* ── Typecheck: structured sources, boundaries, protocol positions ─────── *)

let path_into_structured_source () =
  expect "path into a wire struct" []
    ("struct credentials { token: string, account_id: string }\n"
   ^ "pub struct c {\n\
     \  creds: credentials @env(\"CREDS\")\n\
     \  ep: string @env(\"EP\")\n\
     \  op o(): r @http(method: \"GET\", path: \"/\", endpoint: .ep) \
      @header(\"Authorization\", .creds.token)\n\
      }\n" ^ wire)

let path_into_unknown_member () =
  expect "unknown nested member" [ "TC0038" ]
    ("struct credentials { token: string }\n"
   ^ "pub struct c {\n\
     \  creds: credentials @env(\"CREDS\")\n\
     \  ep: string @env(\"EP\")\n\
     \  op o(): r @http(method: \"GET\", path: \"/\", endpoint: .ep) \
      @header(\"A\", .creds.nope)\n\
      }\n" ^ wire)

let generic_entry_rejected () =
  expect "generic entry" [ "TC0046" ]
    ("pub struct c[t] {\n\
     \  ep: string @env(\"EP\")\n\
     \  op o(): r @http(method: \"GET\", path: \"/\", endpoint: .ep)\n\
      }\n" ^ wire)

let wire_member_of_config_rejected () =
  expect "config inside wire struct" [ "TC0034" ]
    "struct conf { k: string @env(\"K\") }\nstruct w { c: conf }"

let entry_field_of_entry_rejected () =
  (* the entry-typed field also has no source, so both errors report *)
  expect "entry composing an entry" [ "TC0037"; "TC0034" ]
    (entry "  other: c2"
   ^ "pub struct c2 {\n\
     \  ep: string @env(\"EP\")\n\
     \  op p(): r @http(method: \"GET\", path: \"/\", endpoint: .ep)\n\
      }\n")

let union_payload_entry_rejected () =
  expect "entry in a union payload" [ "TC0034" ] (entry "" ^ "union u { v(c) }")

let union_variant_source_rejected () =
  expect "env on a union variant" [ "TC0035" ]
    ("union u { v(r) @env(\"X\") }\n" ^ wire)

let enum_case_source_rejected () =
  expect "arg on an enum case" [ "TC0035" ] "enum e { a @arg, b }"

let union_variant_bind_rejected () =
  expect "bind on a union variant" [ "TC0042" ]
    ("union u { v(r) @bind(a, .b) }\n" ^ wire)

let op_error_entry_rejected () =
  (* TC0015: the entry, named as an error, carries no @status either *)
  expect "entry as a declared error" [ "TC0015"; "TC0034" ]
    (entry "" ^ "op x(): r @errors(c)")

(* The unified request-value grammar accepts a literal endpoint (no
   placeholders needed: the author wrote the whole thing). *)
let endpoint_literal_accepted () =
  expect "literal endpoint" []
    ("pub struct c {\n\
     \  ep: string @env(\"EP\")\n\
     \  op o(): r @http(method: \"GET\", path: \"/\", endpoint: \"https://x\")\n\
      }\n" ^ wire)

let endpoint_non_string_rejected () =
  expect "integer endpoint field" [ "TC0043" ]
    ("pub struct c {\n\
     \  ep: i32 @env(\"EP\")\n\
     \  op o(): r @http(method: \"GET\", path: \"/\", endpoint: .ep)\n\
      }\n" ^ wire)

let op_param_shadows_entry_field_warns () =
  expect "op param shadows entry field" [ "TC0086" ]
    ("pub struct c {\n\
     \  ep: string @env(\"EP\")\n\
     \  op o(ep: string): r @http(method: \"GET\", path: \"/\", endpoint: .ep)\n\
      }\n" ^ wire)

let op_param_distinct_name_no_warning () =
  expect "op param with a distinct name" []
    ("pub struct c {\n\
     \  ep: string @env(\"EP\")\n\
     \  op o(input: string): r @http(method: \"GET\", path: \"/\", endpoint: \
      .ep)\n\
      }\n" ^ wire)

let timeout_wrong_type_rejected () =
  expect "string timeout field" [ "TC0044" ]
    (entry ""
   ^ "pub struct d {\n\
     \  ep: string @env(\"EP\")\n\
     \  op p(): r @http(method: \"GET\", path: \"/\", endpoint: .ep) \
      @timeout(.ep)\n\
      }\n")

let retry_wrong_type_rejected () =
  expect "duration retry field" [ "TC0044" ]
    ("pub struct c {\n\
     \  ep: string @env(\"EP\")\n\
     \  d: duration @with\n\
     \  op o(): r @http(method: \"GET\", path: \"/\", endpoint: .ep) @retry(.d)\n\
      }\n" ^ wire)

let timeout_literal_rejected () =
  expect "literal timeout" [ "TC0044" ]
    ("pub struct c {\n\
     \  ep: string @env(\"EP\")\n\
     \  op o(): r @http(method: \"GET\", path: \"/\", endpoint: .ep) @timeout(5)\n\
      }\n" ^ wire)

let header_arity_rejected () =
  expect "header with one arg" [ "TC0044" ]
    ("pub struct c {\n\
     \  ep: string @env(\"EP\")\n\
     \  op o(): r @http(method: \"GET\", path: \"/\", endpoint: .ep) \
      @header(\"K\")\n\
      }\n" ^ wire)

let header_key_not_string_rejected () =
  expect "non-string header key" [ "TC0044" ]
    ("pub struct c {\n\
     \  ep: string @env(\"EP\")\n\
     \  op o(): r @http(method: \"GET\", path: \"/\", endpoint: .ep) \
      @header(5, .ep)\n\
      }\n" ^ wire)

let path_template_ref_resolves () =
  expect "path with a field placeholder" []
    ("pub struct c {\n\
     \  ep: string @env(\"EP\")\n\
     \  tenant: string @env(\"T\")\n\
     \  op o(): r @http(method: \"GET\", path: \"/{.tenant}/x\", endpoint: .ep)\n\
      }\n" ^ wire)

let path_template_ref_unknown () =
  expect "unknown path placeholder ref" [ "TC0038" ]
    ("pub struct c {\n\
     \  ep: string @env(\"EP\")\n\
     \  op o(): r @http(method: \"GET\", path: \"/{.nope}/x\", endpoint: .ep)\n\
      }\n" ^ wire)

let loose_op_header_literal_ok () =
  expect "loose op with literal header" []
    "struct w { x: string }\n\
     op o(w): w @http(method: \"GET\", path: \"/\") @header(\"K\", \"v\")"

let config_cycle_rejected () =
  expect "cycle inside a config" [ "TC0039" ]
    ("struct conf {\n  a: string @env(.b)\n  b: string @env(.a)\n}\n"
    ^ entry "  s: conf @bind(a, .ep)")

(* ── Remaining branch coverage ─────────────────────────────────────────── *)

let unknown_catalog_rejected () =
  expect "non-str catalog" [ "TC0045" ]
    (entry "  k: string @env(\"K\") @num::abs")

let format_with_sources_dead () =
  expect "format + env" [ "TC0036" ]
    (entry "  k: string @format(\"{.ep}\") @env(\"K\")")

let enum_int_pattern_rejected () =
  expect "int pattern on enum" [ "TC0041"; "TC0040" ]
    ("enum lvl { low, high }\n"
    ^ entry "  l: lvl @env(\"L\")\n  x: string = match .l { 1 => \"a\" }")

let struct_subject_rejected () =
  expect "struct-typed subject" [ "TC0040" ]
    ("struct creds { token: string }\n"
    ^ entry "  c: creds @env(\"C\")\n  x: string = match .c { _ => \"a\" }")

let arm_source_ref_resolves () =
  expect "arm env ref resolves" []
    (entry
       "  other: string @env(\"O\")\n\
       \  v: string @env(\"V\")\n\
       \  x: string = match .v { _ => @env(.other) }")

let path_through_scalar_rejected () =
  expect "path through a scalar field" [ "TC0038" ]
    ("pub struct c {\n\
     \  ep: string @env(\"EP\")\n\
     \  op o(): r @http(method: \"GET\", path: \"/\", endpoint: .ep) \
      @header(\"A\", .ep.x)\n\
      }\n" ^ wire)

let bool_and_name_arm_values_lower () =
  let file, _ =
    Parser.parse
      "struct s {\n\
      \  b: bool @env(\"B\")\n\
      \  x: bool = match .b { true => false false => true }\n\
       }"
  in
  let diags = ref [] in
  let m = Lower.lower_file ~module_name:"m" ~diags file in
  match (List.hd m.shapes).kind with
  | Ir.Config { fields } -> (
      let x = List.nth fields 1 in
      match x.ef_select with
      | Some { arms; _ } ->
          Alcotest.(check bool)
            "bool patterns and values lower as booleans" true
            (List.map (fun (a : Ir.select_arm) -> a.arm_pattern) arms
             = [ Some (`Bool true); Some (`Bool false) ]
            && List.map (fun (a : Ir.select_arm) -> a.arm_value) arms
               = [ Ir.Arm_lit (`Bool false); Ir.Arm_lit (`Bool true) ])
      | None -> Alcotest.fail "no select")
  | _ -> Alcotest.fail "expected a config"

let enum_name_patterns_lower () =
  let file, _ =
    Parser.parse
      "enum lvl { low, high }\n\
       struct s {\n\
      \  l: lvl @env(\"L\")\n\
      \  x: string = match .l { low => low high => \"h\" }\n\
       }"
  in
  let diags = ref [] in
  let m = Lower.lower_file ~module_name:"m" ~diags file in
  let s =
    List.find
      (fun (sh : Ir.shape) ->
        match sh.kind with Ir.Config _ -> true | _ -> false)
      m.shapes
  in
  match s.kind with
  | Ir.Config { fields } -> (
      match (List.nth fields 1).ef_select with
      | Some { arms; _ } ->
          Alcotest.(check bool)
            "enum case names lower as strings" true
            ((List.hd arms).arm_pattern = Some (`String "low")
            && (List.hd arms).arm_value = Ir.Arm_lit (`String "low"))
      | None -> Alcotest.fail "no select")
  | _ -> Alcotest.fail "expected a config"

(* ── Protocol descriptor value forms ───────────────────────────────────── *)

let compile src =
  let file, _ = Parser.parse src in
  let diags = ref [] in
  Lower.lower_file ~module_name:"m" ~diags file

let descriptor_of src =
  let m = Protocol_http.resolve_module (compile src) in
  let entry_shape =
    List.find
      (fun (s : Ir.shape) ->
        match s.kind with Ir.Entry _ -> true | _ -> false)
      m.shapes
  in
  match entry_shape.kind with
  | Ir.Entry { operations; _ } -> (
      let op = List.hd operations in
      match op.kind with
      | Ir.Operation { wire = Some wb; _ } -> wb
      | _ -> Alcotest.fail "no wire binding")
  | _ -> assert false

let header_key_template_and_literal_value () =
  let wb =
    descriptor_of
      ("pub struct c {\n\
       \  ep: string @env(\"EP\")\n\
       \  k: string @env(\"K\")\n\
       \  op o(): r @http(method: \"GET\", path: \"/\", endpoint: .ep) \
        @header(\"X-{.k}\", \"v\") @header(\"Y\", .k)\n\
        }\n" ^ wire)
  in
  match wb.Ir.wb_request_headers with
  | [ (key1, Ir.Wire_lit (`String "v")); (_key2, Ir.Wire_field [ "k" ]) ] ->
      Alcotest.(check bool)
        "templated key has a field part" true
        (match key1 with
        | [ Ir.Tpl_lit _; Ir.Tpl_field _ ] -> true
        | _ -> false)
  | _ -> Alcotest.fail "unexpected request_headers shape"

(* The resolver stays data-driven: a template that reaches a header value in
   hand-authored IR is forwarded, even though the surface rejects it. *)
let resolver_tolerates_value_template () =
  let op : Ir.shape =
    {
      id = "m#c.o";
      kind =
        Ir.Operation
          {
            input = None;
            input_name = None;
            output = None;
            errors = [];
            wire = None;
            impl_call = None;
          };
      traits =
        [
          { Ir.trait_id = "http"; value = `Assoc [ ("method", `String "GET") ] };
          {
            Ir.trait_id = "header";
            value = `List [ `String "K"; `String "{.k}-x" ];
          };
        ];
    }
  in
  match Protocol_http.resolve_op (fun _ -> None) op with
  | Some d -> (
      match d.request_headers with
      | [ (_, Protocol_http.Vtemplate parts) ] ->
          Alcotest.(check int) "template parts" 2 (List.length parts)
      | _ -> Alcotest.fail "expected one templated header value")
  | None -> Alcotest.fail "no descriptor"

let () =
  Alcotest.run "entries_edge"
    [
      ( "parse",
        [
          Alcotest.test_case "= without match" `Quick eq_without_match;
          Alcotest.test_case "subject not ref" `Quick match_subject_not_ref;
          Alcotest.test_case "missing =>" `Quick match_missing_fat_arrow;
          Alcotest.test_case "bad pattern" `Quick match_bad_pattern;
          Alcotest.test_case "bad arm value" `Quick match_bad_value;
          Alcotest.test_case "unterminated match" `Quick match_unterminated;
          Alcotest.test_case "catalog missing name" `Quick catalog_missing_name;
          Alcotest.test_case "ref missing name" `Quick ref_missing_name;
          Alcotest.test_case "struct body junk" `Quick struct_body_junk;
          Alcotest.test_case "kv ref value" `Quick kv_ref_value_parses;
          Alcotest.test_case "ctor field missing name" `Quick
            ctor_field_missing_name;
          Alcotest.test_case "ctor missing closing brace" `Quick
            ctor_missing_closing_brace;
          Alcotest.test_case "ctor empty fields parses" `Quick
            ctor_empty_fields_parses;
        ] );
      ( "fmt",
        [
          Alcotest.test_case "bool and int matches" `Quick
            fmt_bool_and_int_matches;
          Alcotest.test_case "sources arm and ops-only" `Quick
            fmt_sources_arm_and_ops_only;
        ] );
      ( "lower",
        [
          Alcotest.test_case "env bad args" `Quick env_bad_args;
          Alcotest.test_case "format bad args" `Quick format_bad_args;
          Alcotest.test_case "bind bad args" `Quick bind_bad_args;
          Alcotest.test_case "template unterminated" `Quick
            template_unterminated;
          Alcotest.test_case "arm non-source" `Quick arm_with_non_source_trait;
        ] );
      ( "decode",
        [
          Alcotest.test_case "sources" `Quick decode_sources;
          Alcotest.test_case "template parts" `Quick decode_template_parts;
          Alcotest.test_case "arms and select" `Quick
            decode_arm_values_and_select;
          Alcotest.test_case "binds and fields" `Quick decode_binds_and_fields;
        ] );
      ( "match",
        [
          Alcotest.test_case "bool exhaustive" `Quick
            bool_match_exhaustive_without_wildcard;
          Alcotest.test_case "bool bad pattern" `Quick bool_match_bad_pattern;
          Alcotest.test_case "int needs wildcard" `Quick
            int_match_needs_wildcard;
          Alcotest.test_case "int bad pattern" `Quick int_match_bad_pattern;
          Alcotest.test_case "string bad pattern" `Quick
            string_match_bad_pattern;
          Alcotest.test_case "enum full coverage" `Quick
            enum_match_full_coverage;
          Alcotest.test_case "enum unknown case" `Quick enum_match_unknown_case;
          Alcotest.test_case "enum quoted pattern" `Quick
            enum_match_quoted_pattern;
          Alcotest.test_case "subject not scalar" `Quick subject_not_scalar;
          Alcotest.test_case "subject unknown" `Quick subject_unknown;
          Alcotest.test_case "duplicate pattern" `Quick duplicate_pattern;
          Alcotest.test_case "arm after wildcard" `Quick arm_after_wildcard;
          Alcotest.test_case "arm ref unknown" `Quick arm_ref_unknown;
          Alcotest.test_case "arm source ref unknown" `Quick
            arm_source_ref_unknown;
          Alcotest.test_case "arm stacks only sources" `Quick
            arm_stacks_only_sources;
        ] );
      ( "boundaries",
        [
          Alcotest.test_case "structured source path" `Quick
            path_into_structured_source;
          Alcotest.test_case "unknown nested member" `Quick
            path_into_unknown_member;
          Alcotest.test_case "generic entry" `Quick generic_entry_rejected;
          Alcotest.test_case "config in wire member" `Quick
            wire_member_of_config_rejected;
          Alcotest.test_case "entry field of entry" `Quick
            entry_field_of_entry_rejected;
          Alcotest.test_case "entry in union payload" `Quick
            union_payload_entry_rejected;
          Alcotest.test_case "source on union variant" `Quick
            union_variant_source_rejected;
          Alcotest.test_case "source on enum case" `Quick
            enum_case_source_rejected;
          Alcotest.test_case "bind on union variant" `Quick
            union_variant_bind_rejected;
          Alcotest.test_case "entry as declared error" `Quick
            op_error_entry_rejected;
        ] );
      ( "protocol-positions",
        [
          Alcotest.test_case "literal endpoint" `Quick endpoint_literal_accepted;
          Alcotest.test_case "non-string endpoint" `Quick
            endpoint_non_string_rejected;
          Alcotest.test_case "op param shadows entry field" `Quick
            op_param_shadows_entry_field_warns;
          Alcotest.test_case "op param with a distinct name" `Quick
            op_param_distinct_name_no_warning;
          Alcotest.test_case "timeout wrong type" `Quick
            timeout_wrong_type_rejected;
          Alcotest.test_case "retry wrong type" `Quick retry_wrong_type_rejected;
          Alcotest.test_case "timeout literal" `Quick timeout_literal_rejected;
          Alcotest.test_case "header arity" `Quick header_arity_rejected;
          Alcotest.test_case "header key type" `Quick
            header_key_not_string_rejected;
          Alcotest.test_case "path template resolves" `Quick
            path_template_ref_resolves;
          Alcotest.test_case "path template unknown" `Quick
            path_template_ref_unknown;
          Alcotest.test_case "loose literal header" `Quick
            loose_op_header_literal_ok;
          Alcotest.test_case "config cycle" `Quick config_cycle_rejected;
        ] );
      ( "coverage",
        [
          Alcotest.test_case "unknown catalog" `Quick unknown_catalog_rejected;
          Alcotest.test_case "format with sources" `Quick
            format_with_sources_dead;
          Alcotest.test_case "enum int pattern" `Quick enum_int_pattern_rejected;
          Alcotest.test_case "struct subject" `Quick struct_subject_rejected;
          Alcotest.test_case "arm source ref resolves" `Quick
            arm_source_ref_resolves;
          Alcotest.test_case "path through scalar" `Quick
            path_through_scalar_rejected;
          Alcotest.test_case "bool arm lowering" `Quick
            bool_and_name_arm_values_lower;
          Alcotest.test_case "enum arm lowering" `Quick enum_name_patterns_lower;
        ] );
      ( "descriptor",
        [
          Alcotest.test_case "header templates" `Quick
            header_key_template_and_literal_value;
          Alcotest.test_case "resolver value template" `Quick
            resolver_tolerates_value_template;
        ] );
    ]
