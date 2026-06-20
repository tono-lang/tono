open Tono_frontend

let parse_struct_src ?(pub = false) src =
  let toks, ld = Lexer.tokenize src in
  let st = Parser_state.create toks in
  let decl = Parser.parse_struct st ~pub ~dtraits:[] in
  let diags = ref [] in
  let shape = Lower.lower_decl ~diags decl in
  (shape, ld @ Parser_state.diagnostics st @ List.rev !diags)

let shape_of ?(pub = false) src = fst (parse_struct_src ~pub src)
let diags_of src = snd (parse_struct_src src)

let members_of src =
  match (shape_of src).kind with
  | Ir.Structure { members; _ } -> members
  | _ -> Alcotest.fail "expected a structure"

let member name src =
  List.find (fun (m : Ir.member) -> String.equal m.name name) (members_of src)

let tref_str (t : Ir.tref) = Ir_json.to_canonical_string (Ir_json.encode_tref t)

let cons_str (c : Ir.constraint_) =
  Ir_json.to_canonical_string (Ir_json.encode_constraint c)

(* ── Structs and members ───────────────────────────────────────────────── *)

let struct_golden () =
  let ms =
    members_of "struct charge { id: uuid\n amount: i64\n note: string? }"
  in
  Alcotest.(check int) "member count" 3 (List.length ms);
  let id =
    member "id" "struct charge { id: uuid\n amount: i64\n note: string? }"
  in
  Alcotest.(check string) "id target" {|{"prim":"uuid"}|} (tref_str id.target);
  Alcotest.(check bool) "id required" true id.required

let nullable_two_state () =
  let src =
    "struct s { a: string\n b: string?\n c: []charge?\n d: map[string]i64? }"
  in
  let req name = (member name src).required in
  Alcotest.(check bool) "a required" true (req "a");
  Alcotest.(check bool) "b nullable" false (req "b");
  Alcotest.(check bool) "c nullable" false (req "c");
  Alcotest.(check bool) "d nullable" false (req "d");
  (* The element type survives; only the member flag changes. *)
  Alcotest.(check string)
    "c target is a list" {|{"list":{"args":[],"ref":"charge"}}|}
    (tref_str (member "c" src).target)

let required_and_default () =
  let src = "struct s { a: i64 @required\n b: u32 @default(1)\n c: string }" in
  Alcotest.(check bool) "a required" true (member "a" src).required;
  let b = member "b" src in
  Alcotest.(check bool) "b required (default <> nullable)" true b.required;
  Alcotest.(check string)
    "b default" "1"
    (match b.default with
    | Some v -> Ir_json.to_canonical_string v
    | None -> "<none>")

let core_constraints_lifted () =
  let src =
    {|struct s { a: i64 @range(min: 0, max: 100)
       n: string @length(max: 255) @pattern("^x$")
       m: i64 @multipleOf(5) }|}
  in
  let a = member "a" src in
  Alcotest.(check int) "a has one constraint" 1 (List.length a.constraints);
  Alcotest.(check string)
    "a range"
    {|{"range":{"exclMax":false,"exclMin":false,"max":100.0,"min":0.0}}|}
    (cons_str (List.hd a.constraints));
  let n = member "n" src in
  Alcotest.(check int) "n has two constraints" 2 (List.length n.constraints);
  Alcotest.(check int) "constraints not in bag" 0 (List.length n.traits);
  let m = member "m" src in
  Alcotest.(check string)
    "m multipleOf" {|{"multipleOf":5.0}|}
    (cons_str (List.hd m.constraints))

let noncore_traits_in_bag () =
  let src = {|struct s { a: string @wire("amount") @deprecated @doc("hi") }|} in
  let a = member "a" src in
  Alcotest.(check int) "no constraints" 0 (List.length a.constraints);
  let ids = List.map (fun (t : Ir.trait) -> t.trait_id) a.traits in
  Alcotest.(check (list string))
    "bag trait ids"
    [ "core#wire"; "core#deprecated"; "core#doc" ]
    ids;
  let wire = List.hd a.traits in
  Alcotest.(check string)
    "wire value" {|["amount"]|}
    (Ir_json.to_canonical_string wire.value)

(* Bag traits keep every argument form: bare names, key:string and key:name. *)
let bag_arg_forms () =
  let src =
    {|struct s { a: i64 @flag(verbose) @meta(owner: "me") @cfg(mode: fast) }|}
  in
  let a = member "a" src in
  let value id =
    let t = List.find (fun (t : Ir.trait) -> t.trait_id = id) a.traits in
    Ir_json.to_canonical_string t.value
  in
  Alcotest.(check string) "bare name arg" {|["verbose"]|} (value "core#flag");
  Alcotest.(check string)
    "kv string arg" {|[{"owner":"me"}]|} (value "core#meta");
  Alcotest.(check string) "kv name arg" {|[{"mode":"fast"}]|} (value "core#cfg")

(* Constraints also accept positional [min, max] bounds. *)
let positional_bounds () =
  let src = "struct s { a: i64 @range(0, 100)\n b: string @length(1, 5) }" in
  Alcotest.(check string)
    "positional range"
    {|{"range":{"exclMax":false,"exclMin":false,"max":100.0,"min":0.0}}|}
    (cons_str (List.hd (member "a" src).constraints));
  Alcotest.(check string)
    "positional length" {|{"length":{"max":5,"min":1}}|}
    (cons_str (List.hd (member "b" src).constraints))

(* Ill-typed constraint arguments are diagnosed and the constraint is dropped. *)
let constraint_arg_errors () =
  let src =
    {|struct s { a: string @pattern(5)
       b: i64 @multipleOf("x") }|}
  in
  Alcotest.(check bool) "two diagnostics" true (List.length (diags_of src) >= 2);
  Alcotest.(check int)
    "pattern dropped" 0
    (List.length (member "a" src).constraints);
  Alcotest.(check int)
    "multipleOf dropped" 0
    (List.length (member "b" src).constraints)

(* ── Generics ──────────────────────────────────────────────────────────── *)

let generic_param_use () =
  let src = "struct page[T] { items: []T\n next: string? }" in
  (match (shape_of src).kind with
  | Ir.Structure { params; _ } ->
      Alcotest.(check (list string)) "params" [ "T" ] params
  | _ -> Alcotest.fail "expected a structure");
  Alcotest.(check string)
    "items is list of param" {|{"list":{"param":"T"}}|}
    (tref_str (member "items" src).target)

(* ── Visibility ────────────────────────────────────────────────────────── *)

let pub_trait () =
  let s = shape_of ~pub:true "struct charge { a: i64 }" in
  Alcotest.(check bool)
    "pub recorded" true
    (List.exists (fun (t : Ir.trait) -> t.trait_id = "core#pub") s.traits)

(* ── snake_case ────────────────────────────────────────────────────────── *)

let snake_case_checks () =
  Alcotest.(check bool)
    "PascalCase shape diagnosed" true
    (List.length (diags_of "struct Charge { a: i64 }") >= 1);
  Alcotest.(check bool)
    "camelCase member diagnosed" true
    (List.length (diags_of "struct charge { amountCents: i64 }") >= 1);
  Alcotest.(check int)
    "clean struct has no diagnostics" 0
    (List.length (diags_of "struct charge { amount_cents: i64 }"))

let () =
  Alcotest.run "struct"
    [
      ( "lower",
        [
          Alcotest.test_case "struct golden" `Quick struct_golden;
          Alcotest.test_case "nullable two-state" `Quick nullable_two_state;
          Alcotest.test_case "required and default" `Quick required_and_default;
          Alcotest.test_case "core constraints lifted" `Quick
            core_constraints_lifted;
          Alcotest.test_case "non-core traits in bag" `Quick
            noncore_traits_in_bag;
          Alcotest.test_case "bag arg forms" `Quick bag_arg_forms;
          Alcotest.test_case "positional bounds" `Quick positional_bounds;
          Alcotest.test_case "constraint arg errors" `Quick
            constraint_arg_errors;
          Alcotest.test_case "generic param use" `Quick generic_param_use;
          Alcotest.test_case "pub trait" `Quick pub_trait;
          Alcotest.test_case "snake_case checks" `Quick snake_case_checks;
        ] );
    ]
