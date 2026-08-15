(* Negative and edge-case decode coverage: every decoder error branch, every
   tolerant default, and the integer/float coercion helpers. *)

open Tono_frontend

let parse = Yojson.Safe.from_string

let fails name decode src =
  Alcotest.test_case name `Quick (fun () ->
      match decode (parse src) with
      | Error _ -> ()
      | Ok _ -> Alcotest.failf "%s: expected a decode error" name)

let ok name decode src =
  Alcotest.test_case name `Quick (fun () ->
      match decode (parse src) with
      | Ok _ -> ()
      | Error e -> Alcotest.failf "%s: expected success, got %s" name e)

(* ── Type references ───────────────────────────────────────────────────── *)

let tref_suite =
  [
    fails "tref not an object" Ir_json.decode_tref {|"nope"|};
    fails "tref map value not array" Ir_json.decode_tref {|{"map": 5}|};
    fails "tref map wrong arity" Ir_json.decode_tref
      {|{"map": [{"prim":"bool"}]}|};
    fails "tref ref id not string" Ir_json.decode_tref
      {|{"ref": 5, "args": []}|};
    fails "tref ref args not array" Ir_json.decode_tref
      {|{"ref": "x#Y", "args": 5}|};
    fails "tref prim not string" Ir_json.decode_tref {|{"prim": 5}|};
    fails "tref list extra key" Ir_json.decode_tref
      {|{"list": {"prim":"bool"}, "extra": 1}|};
    fails "tref map extra key" Ir_json.decode_tref
      {|{"map": [{"prim":"bool"},{"prim":"bool"}], "x": 1}|};
    fails "tref param extra key" Ir_json.decode_tref {|{"param": "T", "x": 1}|};
  ]

(* ── Constraints ───────────────────────────────────────────────────────── *)

let constraint_suite =
  [
    fails "constraint not an object" Ir_json.decode_constraint {|5|};
    fails "range value not object" Ir_json.decode_constraint {|{"range": 5}|};
    fails "range min not number" Ir_json.decode_constraint
      {|{"range": {"min": "x"}}|};
    fails "range max not number" Ir_json.decode_constraint
      {|{"range": {"max": "x"}}|};
    fails "length value not object" Ir_json.decode_constraint {|{"length": 5}|};
    fails "length min not integer" Ir_json.decode_constraint
      {|{"length": {"min": "x"}}|};
    fails "length min out of range" Ir_json.decode_constraint
      {|{"length": {"min": 99999999999999999999}}|};
    fails "pattern not string" Ir_json.decode_constraint {|{"pattern": 5}|};
    fails "multipleOf not number" Ir_json.decode_constraint
      {|{"multipleOf": "x"}|};
    fails "range bound not finite" Ir_json.decode_constraint
      {|{"range": {"min": 1e999}}|};
    fails "multipleOf not finite" Ir_json.decode_constraint
      {|{"multipleOf": 1e999}|};
    fails "constraint extra sibling key" Ir_json.decode_constraint
      {|{"pattern": "x", "bogus": 1}|};
    fails "range exclMin wrong type" Ir_json.decode_constraint
      {|{"range": {"exclMin": "yes"}}|};
    ok "range without excl flags defaults false" Ir_json.decode_constraint
      {|{"range": {"min": 1}}|};
    ok "length empty bounds" Ir_json.decode_constraint {|{"length": {}}|};
  ]

let range_excl_defaults () =
  match Ir_json.decode_constraint (parse {|{"range": {"min": 1}}|}) with
  | Ok (Ir.Range { excl_min; excl_max; _ }) ->
      Alcotest.(check bool) "exclMin defaults false" false excl_min;
      Alcotest.(check bool) "exclMax defaults false" false excl_max
  | _ -> Alcotest.fail "expected a range"

(* ── Traits ────────────────────────────────────────────────────────────── *)

let trait_suite =
  [
    fails "trait not an object" Ir_json.decode_trait {|"x"|};
    fails "trait missing id" Ir_json.decode_trait {|{"value": 1}|};
    fails "trait missing value" Ir_json.decode_trait {|{"id": "core#x"}|};
    fails "trait id not string" Ir_json.decode_trait {|{"id": 5, "value": 1}|};
  ]

(* ── Members ───────────────────────────────────────────────────────────── *)

let member_suite =
  [
    fails "member not an object" Ir_json.decode_member {|5|};
    fails "member missing name" Ir_json.decode_member
      {|{"target": {"prim":"bool"}, "required": true}|};
    fails "member missing target" Ir_json.decode_member
      {|{"name": "x", "required": true}|};
    fails "member missing required" Ir_json.decode_member
      {|{"name": "x", "target": {"prim":"bool"}}|};
    fails "member name not string" Ir_json.decode_member
      {|{"name": 5, "target": {"prim":"bool"}, "required": true}|};
    fails "member required not bool" Ir_json.decode_member
      {|{"name": "x", "target": {"prim":"bool"}, "required": 5}|};
    fails "member constraints not array" Ir_json.decode_member
      {|{"name": "x", "target": {"prim":"bool"}, "required": true, "constraints": 5}|};
    fails "member traits not array" Ir_json.decode_member
      {|{"name": "x", "target": {"prim":"bool"}, "required": true, "traits": 5}|};
    ok "member minimal (defaults for arrays)" Ir_json.decode_member
      {|{"name": "x", "target": {"prim":"bool"}, "required": false}|};
  ]

(* ── Shapes ────────────────────────────────────────────────────────────── *)

let shape_suite =
  [
    fails "shape not an object" Ir_json.decode_shape {|5|};
    fails "shape missing id" Ir_json.decode_shape {|{"kind": "structure"}|};
    fails "shape id not string" Ir_json.decode_shape
      {|{"id": 5, "kind": "structure"}|};
    fails "shape missing kind" Ir_json.decode_shape {|{"id": "x#Y"}|};
    fails "shape unknown kind" Ir_json.decode_shape
      {|{"id": "x#Y", "kind": "frobnicate"}|};
    fails "structure params not array" Ir_json.decode_shape
      {|{"id": "x#Y", "kind": "structure", "params": 5}|};
    fails "structure members not array" Ir_json.decode_shape
      {|{"id": "x#Y", "kind": "structure", "members": 5}|};
    fails "structure traits not array" Ir_json.decode_shape
      {|{"id": "x#Y", "kind": "structure", "traits": 5}|};
    fails "union discriminator wrong type" Ir_json.decode_shape
      {|{"id": "x#U", "kind": "union", "discriminator": 5}|};
    fails "enum missing backing" Ir_json.decode_shape
      {|{"id": "x#Y", "kind": "enum"}|};
    fails "enum bad backing" Ir_json.decode_shape
      {|{"id": "x#Y", "kind": "enum", "backing": "float"}|};
    fails "enum backing not string" Ir_json.decode_shape
      {|{"id": "x#Y", "kind": "enum", "backing": 5}|};
    fails "enum values not array" Ir_json.decode_shape
      {|{"id": "x#Y", "kind": "enum", "backing": "string", "values": 5}|};
    fails "enum value not object" Ir_json.decode_shape
      {|{"id": "x#Y", "kind": "enum", "backing": "int", "values": [5]}|};
    fails "enum value missing name" Ir_json.decode_shape
      {|{"id": "x#Y", "kind": "enum", "backing": "int", "values": [{"value": 1}]}|};
    fails "enum value name not string" Ir_json.decode_shape
      {|{"id": "x#Y", "kind": "enum", "backing": "int", "values": [{"name": 5}]}|};
    fails "enum value traits not array" Ir_json.decode_shape
      {|{"id": "x#Y", "kind": "enum", "backing": "string", "values": [{"name": "a", "traits": 5}]}|};
    fails "service operations not array" Ir_json.decode_shape
      {|{"id": "x#Y", "kind": "service", "operations": 5}|};
    fails "operation errors not array" Ir_json.decode_shape
      {|{"id": "x#Y", "kind": "operation", "errors": 5}|};
    fails "operation input not a tref" Ir_json.decode_shape
      {|{"id": "x#Y", "kind": "operation", "input": 5}|};
    ok "service without operations" Ir_json.decode_shape
      {|{"id": "x#S", "kind": "service"}|};
    ok "union without discriminator" Ir_json.decode_shape
      {|{"id": "x#U", "kind": "union"}|};
    ok "enum without open flag" Ir_json.decode_shape
      {|{"id": "x#E", "kind": "enum", "backing": "string"}|};
    ok "operation null io" Ir_json.decode_shape
      {|{"id": "x#O", "kind": "operation", "input": null, "output": null}|};
    ok "operation absent io" Ir_json.decode_shape
      {|{"id": "x#O", "kind": "operation"}|};
    ok "operation null wire" Ir_json.decode_shape
      {|{"id": "x#O", "kind": "operation", "wire": null}|};
    ok "operation with wire" Ir_json.decode_shape
      {|{"id": "x#O", "kind": "operation", "wire": {"method": "GET"}}|};
    fails "operation malformed wire" Ir_json.decode_shape
      {|{"id": "x#O", "kind": "operation", "wire": {"method": 5}}|};
  ]

(* ── Resolved wire bindings ────────────────────────────────────────────── *)

let wire_response_part_suite =
  [
    fails "wire response part not an object" Ir_json.decode_wire_response_part
      {|5|};
    fails "wire response part missing kind" Ir_json.decode_wire_response_part
      {|{}|};
    fails "wire response part unknown kind" Ir_json.decode_wire_response_part
      {|{"kind": "bogus"}|};
    fails "wire response part header missing name"
      Ir_json.decode_wire_response_part {|{"kind": "header"}|};
    ok "wire response part header" Ir_json.decode_wire_response_part
      {|{"kind": "header", "name": "X-Trace-Id"}|};
    ok "wire response part status code" Ir_json.decode_wire_response_part
      {|{"kind": "statusCode"}|};
  ]

let wire_value_suite =
  [
    fails "wire value not an object" Ir_json.decode_wire_value {|5|};
    fails "wire value empty" Ir_json.decode_wire_value {|{}|};
    fails "wire value multiple keys" Ir_json.decode_wire_value
      {|{"lit": 1, "field": ["a"]}|};
    fails "wire value unknown key" Ir_json.decode_wire_value {|{"bogus": 1}|};
    ok "wire value lit" Ir_json.decode_wire_value {|{"lit": 5}|};
    ok "wire value field" Ir_json.decode_wire_value {|{"field": ["a", "b"]}|};
    ok "wire value template" Ir_json.decode_wire_value
      {|{"template": [{"lit": "x"}]}|};
    fails "wire value object bad field" Ir_json.decode_wire_value
      {|{"object": [["a", 5]]}|};
    ok "wire value object" Ir_json.decode_wire_value
      {|{"object": [["a", {"lit": 1}], ["b", {"field": ["x"]}]]}|};
    fails "wire value call missing ns" Ir_json.decode_wire_value
      {|{"call": {"fn": "sign", "args": []}}|};
    fails "wire value call missing fn" Ir_json.decode_wire_value
      {|{"call": {"ns": "companyauth", "args": []}}|};
    fails "wire value call bad arg" Ir_json.decode_wire_value
      {|{"call": {"ns": "companyauth", "fn": "sign", "args": [5]}}|};
    fails "wire value call ctor arg not a pair" Ir_json.decode_wire_value
      {|{"call": {"ns": "companyauth", "fn": "sign", "args": [{"ctor": [5]}]}}|};
    ok "wire value call with no args" Ir_json.decode_wire_value
      {|{"call": {"ns": "companyauth", "fn": "sign", "args": []}}|};
    ok "wire value call with every arg tag" Ir_json.decode_wire_value
      {|{"call": {"ns": "companyauth", "fn": "sign", "args": [
          "request",
          {"field": ["id"]},
          {"param": ["region"]},
          {"lit": "v"},
          {"ctor": [["signature", {"field": ["sig"]}]]}
        ]}}|};
  ]

(* Every encode_wire_call_arg tag round-trips through decode unchanged,
   proving the encoder's own output (not just hand-written decode fixtures
   above) stays parseable. *)
let wire_call_round_trip_suite =
  [
    Alcotest.test_case "wire call round-trips every argument tag" `Quick
      (fun () ->
        let call : Ir.wire_call =
          {
            wcl_ns = "companyauth";
            wcl_fn = "sign";
            wcl_args =
              [
                Ir.Wca_request;
                Ir.Wca_field [ "id" ];
                Ir.Wca_param [ "region" ];
                Ir.Wca_lit (`String "v");
                Ir.Wca_ctor [ ("signature", Ir.Wca_field [ "sig" ]) ];
              ];
          }
        in
        let value = Ir.Wire_call call in
        let json = Ir_json.encode_wire_value value in
        match Ir_json.decode_wire_value json with
        | Error e -> Alcotest.failf "decode failed: %s" e
        | Ok decoded ->
            Alcotest.(check bool) "round-trips" true (decoded = value));
  ]

let wire_binding_suite =
  [
    fails "wire binding not an object" Ir_json.decode_wire_binding {|5|};
    fails "wire binding missing method" Ir_json.decode_wire_binding {|{}|};
    fails "wire binding method not string" Ir_json.decode_wire_binding
      {|{"method": 5}|};
    fails "wire binding uri not array" Ir_json.decode_wire_binding
      {|{"method": "GET", "uri": 5}|};
    fails "wire binding body malformed" Ir_json.decode_wire_binding
      {|{"method": "GET", "body": 5}|};
    fails "wire binding response_bindings not object"
      Ir_json.decode_wire_binding {|{"method": "GET", "response_bindings": 5}|};
    fails "wire binding success not array" Ir_json.decode_wire_binding
      {|{"method": "GET", "success": 5}|};
    fails "wire binding endpoint not array of strings"
      Ir_json.decode_wire_binding {|{"method": "GET", "endpoint": [5]}|};
    fails "wire binding request_headers malformed" Ir_json.decode_wire_binding
      {|{"method": "GET", "request_headers": [5]}|};
    fails "wire binding timeout not array" Ir_json.decode_wire_binding
      {|{"method": "GET", "timeout": 5}|};
    fails "wire binding retry not array" Ir_json.decode_wire_binding
      {|{"method": "GET", "retry": 5}|};
    ok "wire binding minimal" Ir_json.decode_wire_binding {|{"method": "GET"}|};
    ok "wire binding full" Ir_json.decode_wire_binding
      {|{
          "method": "POST",
          "uri": {"template": [{"lit": "/charges/"}, {"input": "id"}]},
          "body": {"object": [["id", {"param": []}]]},
          "response_bindings": {"trace_id": {"kind": "header", "name": "X-Trace-Id"}},
          "success": [200, 202],
          "endpoint": {"field": ["endpoint"]},
          "request_headers": [[[{"lit": "X-Client"}], {"field": ["client_name"]}]],
          "query": [[[{"lit": "limit"}], {"field": ["default_limit"]}]],
          "timeout": ["timeout"],
          "retry": ["settings", "max_retries"]
        }|};
  ]

let wire_suite =
  wire_response_part_suite @ wire_value_suite @ wire_binding_suite
  @ wire_call_round_trip_suite

let union_discriminator_defaults () =
  match Ir_json.decode_shape (parse {|{"id": "x#U", "kind": "union"}|}) with
  | Ok { kind = Ir.Union { discriminator; _ }; _ } ->
      Alcotest.(check string)
        "discriminator defaults to type" "type" discriminator
  | _ -> Alcotest.fail "expected a union"

(* ── Modules / model ───────────────────────────────────────────────────── *)

let model_suite =
  [
    fails "module not an object" Ir_json.decode_module {|5|};
    fails "module missing name" Ir_json.decode_module {|{"shapes": []}|};
    fails "module name not string" Ir_json.decode_module {|{"name": 5}|};
    fails "module shapes not array" Ir_json.decode_module
      {|{"name": "m", "shapes": 5}|};
    fails "model not an object" Ir_json.decode_model {|5|};
    fails "model version not integer" Ir_json.decode_model
      {|{"tono_ir_version": "x", "modules": []}|};
    fails "model modules not array" Ir_json.decode_model
      {|{"tono_ir_version": 3, "modules": 5}|};
    ok "model without modules" Ir_json.decode_model
      (Printf.sprintf {|{"tono_ir_version": %d}|} Ir_json.current_ir_version);
    ok "module minimal" Ir_json.decode_module {|{"name": "m"}|};
  ]

(* ── Coercion helpers and encoder guards ───────────────────────────────── *)

let helper_coercions () =
  let check_ok name = function
    | Ok _ -> ()
    | Error e -> Alcotest.failf "%s: %s" name e
  in
  let check_err name = function
    | Error _ -> ()
    | Ok _ -> Alcotest.failf "%s: expected error" name
  in
  check_ok "as_int intlit fits" (Ir_json.Internal.as_int (`Intlit "5"));
  check_err "as_int intlit overflow"
    (Ir_json.Internal.as_int (`Intlit "99999999999999999999"));
  check_err "as_int non-integer" (Ir_json.Internal.as_int (`Bool true));
  (match Ir_json.Internal.as_float (`Intlit "5") with
  | Ok f -> Alcotest.(check (float 0.)) "as_float intlit" 5.0 f
  | Error e -> Alcotest.failf "as_float intlit: %s" e);
  check_err "as_float not-a-number intlit"
    (Ir_json.Internal.as_float (`Intlit "abc"));
  check_err "as_float non-number" (Ir_json.Internal.as_float (`Bool true))

let canonicalize_collapses_intlit () =
  Alcotest.(check string)
    "small intlit collapses to int" "5"
    (Ir_json.to_canonical_string (`Intlit "5"));
  Alcotest.(check string)
    "huge intlit preserved" "99999999999999999999"
    (Ir_json.to_canonical_string (`Intlit "99999999999999999999"))

let encode_rejects_bad_int_width () =
  Alcotest.(check bool)
    "encode_prim rejects width 33" true
    (try
       ignore (Ir_json.encode_prim (Ir.Int { bits = 33; signed = true }));
       false
     with Ir.Invalid_ir _ -> true)

let helper_suite =
  [
    Alcotest.test_case "coercion helpers" `Quick helper_coercions;
    Alcotest.test_case "canonicalize intlit" `Quick
      canonicalize_collapses_intlit;
    Alcotest.test_case "encode rejects bad int width" `Quick
      encode_rejects_bad_int_width;
    Alcotest.test_case "range excl defaults" `Quick range_excl_defaults;
    Alcotest.test_case "union discriminator default" `Quick
      union_discriminator_defaults;
  ]

let () =
  Alcotest.run "decode"
    [
      ("tref", tref_suite);
      ("constraint", constraint_suite);
      ("trait", trait_suite);
      ("member", member_suite);
      ("shape", shape_suite);
      ("wire", wire_suite);
      ("model", model_suite);
      ("helpers", helper_suite);
    ]
