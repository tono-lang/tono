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
    fails "enum open wrong type" Ir_json.decode_shape
      {|{"id": "x#E", "kind": "enum", "backing": "string", "open": "yes"}|};
    fails "enum missing backing" Ir_json.decode_shape
      {|{"id": "x#Y", "kind": "enum"}|};
    fails "enum bad backing" Ir_json.decode_shape
      {|{"id": "x#Y", "kind": "enum", "backing": "float"}|};
    fails "enum backing not string" Ir_json.decode_shape
      {|{"id": "x#Y", "kind": "enum", "backing": 5}|};
    fails "enum values not array" Ir_json.decode_shape
      {|{"id": "x#Y", "kind": "enum", "backing": "string", "values": 5}|};
    fails "enum value not pair" Ir_json.decode_shape
      {|{"id": "x#Y", "kind": "enum", "backing": "int", "values": [["a",1,2]]}|};
    fails "enum value entry not array" Ir_json.decode_shape
      {|{"id": "x#Y", "kind": "enum", "backing": "int", "values": [5]}|};
    fails "enum value name not string" Ir_json.decode_shape
      {|{"id": "x#Y", "kind": "enum", "backing": "int", "values": [[5, 1]]}|};
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
  ]

let union_discriminator_defaults () =
  match Ir_json.decode_shape (parse {|{"id": "x#U", "kind": "union"}|}) with
  | Ok { kind = Ir.Union { discriminator; _ }; _ } ->
      Alcotest.(check string)
        "discriminator defaults to type" "type" discriminator
  | _ -> Alcotest.fail "expected a union"

let enum_open_defaults () =
  match
    Ir_json.decode_shape
      (parse {|{"id": "x#E", "kind": "enum", "backing": "string"}|})
  with
  | Ok { kind = Ir.Enum { open_; _ }; _ } ->
      Alcotest.(check bool) "open defaults true" true open_
  | _ -> Alcotest.fail "expected an enum"

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
      {|{"tono_ir_version": 1, "modules": 5}|};
    ok "model without modules" Ir_json.decode_model {|{"tono_ir_version": 1}|};
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
    Alcotest.test_case "enum open default" `Quick enum_open_defaults;
  ]

let () =
  Alcotest.run "decode"
    [
      ("tref", tref_suite);
      ("constraint", constraint_suite);
      ("trait", trait_suite);
      ("member", member_suite);
      ("shape", shape_suite);
      ("model", model_suite);
      ("helpers", helper_suite);
    ]
