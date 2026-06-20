open Tono_frontend

let run ?(dtraits = []) ?(pub = false) parse src =
  let toks, ld = Lexer.tokenize src in
  let st = Parser_state.create toks in
  let decl = parse st ~pub ~dtraits in
  let diags = ref [] in
  let shape = Lower.lower_decl ~diags decl in
  (shape, ld @ Parser_state.diagnostics st @ List.rev !diags)

let shape_json shape = Ir_json.to_canonical_string (Ir_json.encode_shape shape)
let tref_str t = Ir_json.to_canonical_string (Ir_json.encode_tref t)
let tref_opt = function None -> "<none>" | Some t -> tref_str t

let trait_ids (s : Ir.shape) =
  List.map (fun (t : Ir.trait) -> t.trait_id) s.traits

(* Parse a single leading trait, e.g. "@open" or "@discriminator(\"k\")", so tests
   can supply the shape-level traits the file-level parser will later collect. *)
let one_trait src =
  Parser.parse_trait (Parser_state.create (fst (Lexer.tokenize src)))

(* ── Enum ──────────────────────────────────────────────────────────────── *)

let enum_string_backed () =
  let shape, ds = run Parser.parse_enum "enum status { active, closed }" in
  Alcotest.(check int) "no diagnostics" 0 (List.length ds);
  Alcotest.(check string)
    "string enum"
    {|{"backing":"string","id":"status","kind":"enum","open":false,"traits":[],"values":[["active",null],["closed",null]]}|}
    (shape_json shape)

let enum_int_backed () =
  let shape, ds =
    run Parser.parse_enum "enum http_code { ok = 200, fail = 500 }"
  in
  Alcotest.(check int) "no diagnostics" 0 (List.length ds);
  Alcotest.(check string)
    "int enum"
    {|{"backing":"int","id":"http_code","kind":"enum","open":false,"traits":[],"values":[["ok",200],["fail",500]]}|}
    (shape_json shape)

let enum_open () =
  let shape, ds =
    run
      ~dtraits:[ one_trait "@open" ]
      Parser.parse_enum "enum currency { usd, eur }"
  in
  Alcotest.(check int) "no diagnostics" 0 (List.length ds);
  Alcotest.(check string)
    "open enum"
    {|{"backing":"string","id":"currency","kind":"enum","open":true,"traits":[],"values":[["usd",null],["eur",null]]}|}
    (shape_json shape)

let enum_int_missing_value () =
  let _, ds = run Parser.parse_enum "enum mixed { a = 1, b }" in
  Alcotest.(check bool) "missing int value diagnosed" true (List.length ds >= 1)

let enum_case_snake () =
  let _, ds = run Parser.parse_enum "enum e { Active }" in
  Alcotest.(check bool) "PascalCase case diagnosed" true (List.length ds >= 1)

let enum_case_traits_rejected () =
  let _, ds = run Parser.parse_enum {|enum e { a @doc("x") }|} in
  Alcotest.(check bool) "case traits diagnosed" true (List.length ds >= 1)

(* ── Union ─────────────────────────────────────────────────────────────── *)

let union_basic () =
  let shape, ds =
    run Parser.parse_union "union source { card: card\n bank: bank_account }"
  in
  Alcotest.(check int) "no diagnostics" 0 (List.length ds);
  match shape.kind with
  | Ir.Union { params; members; discriminator } ->
      Alcotest.(check string) "default discriminator" "type" discriminator;
      Alcotest.(check (list string)) "no params" [] params;
      Alcotest.(check (list string))
        "member names" [ "card"; "bank" ]
        (List.map (fun (m : Ir.member) -> m.name) members);
      Alcotest.(check string)
        "card target" {|{"args":[],"ref":"card"}|}
        (tref_str (List.hd members).target)
  | _ -> Alcotest.fail "expected a union"

let union_generics () =
  let shape, _ =
    run Parser.parse_union "union box[t] { some: t\n none: unit }"
  in
  match shape.kind with
  | Ir.Union { params; members; _ } ->
      Alcotest.(check (list string)) "params" [ "t" ] params;
      Alcotest.(check string)
        "param member target" {|{"param":"t"}|}
        (tref_str (List.hd members).target)
  | _ -> Alcotest.fail "expected a union"

let union_discriminator_and_bag () =
  let shape, _ =
    run
      ~dtraits:
        [ one_trait {|@doc("hi")|}; one_trait {|@discriminator("kind")|} ]
      Parser.parse_union "union event { a: a\n b: b }"
  in
  (match shape.kind with
  | Ir.Union { discriminator; _ } ->
      Alcotest.(check string) "custom discriminator" "kind" discriminator
  | _ -> Alcotest.fail "expected a union");
  Alcotest.(check (list string))
    "discriminator consumed, doc kept" [ "core#doc" ] (trait_ids shape)

let union_bad_discriminator () =
  let _, ds =
    run
      ~dtraits:[ one_trait "@discriminator(5)" ]
      Parser.parse_union "union u { a: a }"
  in
  Alcotest.(check bool)
    "non-string discriminator diagnosed" true
    (List.length ds >= 1)

(* ── Operation ─────────────────────────────────────────────────────────── *)

let op_full () =
  let shape, ds =
    run Parser.parse_op
      "op create_charge(create_charge_input) -> charge throws not_found, \
       rate_limited"
  in
  Alcotest.(check int) "no diagnostics" 0 (List.length ds);
  match shape.kind with
  | Ir.Operation { input; output; errors } ->
      Alcotest.(check string)
        "input" {|{"args":[],"ref":"create_charge_input"}|} (tref_opt input);
      Alcotest.(check string)
        "output" {|{"args":[],"ref":"charge"}|} (tref_opt output);
      Alcotest.(check (list string))
        "errors"
        [
          {|{"args":[],"ref":"not_found"}|};
          {|{"args":[],"ref":"rate_limited"}|};
        ]
        (List.map tref_str errors)
  | _ -> Alcotest.fail "expected an operation"

let op_no_input () =
  let shape, _ = run Parser.parse_op "op list_charges() -> page[charge]" in
  match shape.kind with
  | Ir.Operation { input; output; errors } ->
      Alcotest.(check string) "no input" "<none>" (tref_opt input);
      Alcotest.(check string)
        "output" {|{"args":[{"args":[],"ref":"charge"}],"ref":"page"}|}
        (tref_opt output);
      Alcotest.(check int) "no errors" 0 (List.length errors)
  | _ -> Alcotest.fail "expected an operation"

let op_no_output () =
  let shape, _ = run Parser.parse_op "op fire(event)" in
  match shape.kind with
  | Ir.Operation { input; output; errors } ->
      Alcotest.(check string)
        "input" {|{"args":[],"ref":"event"}|} (tref_opt input);
      Alcotest.(check string) "no output" "<none>" (tref_opt output);
      Alcotest.(check int) "no errors" 0 (List.length errors)
  | _ -> Alcotest.fail "expected an operation"

let () =
  Alcotest.run "decl"
    [
      ( "enum",
        [
          Alcotest.test_case "string backed" `Quick enum_string_backed;
          Alcotest.test_case "int backed" `Quick enum_int_backed;
          Alcotest.test_case "open" `Quick enum_open;
          Alcotest.test_case "int missing value" `Quick enum_int_missing_value;
          Alcotest.test_case "case snake_case" `Quick enum_case_snake;
          Alcotest.test_case "case traits rejected" `Quick
            enum_case_traits_rejected;
        ] );
      ( "union",
        [
          Alcotest.test_case "basic" `Quick union_basic;
          Alcotest.test_case "generics" `Quick union_generics;
          Alcotest.test_case "discriminator and bag" `Quick
            union_discriminator_and_bag;
          Alcotest.test_case "bad discriminator" `Quick union_bad_discriminator;
        ] );
      ( "operation",
        [
          Alcotest.test_case "full signature" `Quick op_full;
          Alcotest.test_case "no input" `Quick op_no_input;
          Alcotest.test_case "no output" `Quick op_no_output;
        ] );
    ]
