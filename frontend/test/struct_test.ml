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
  (* lower_decl leaves trait ids bare; module resolution qualifies them. *)
  Alcotest.(check (list string))
    "bag trait ids"
    [ "wire"; "deprecated"; "doc" ]
    ids;
  let wire = List.hd a.traits in
  Alcotest.(check string)
    "wire value" {|["amount"]|}
    (Ir_json.to_canonical_string wire.value)

(* Bag trait values: positional args form an array; all-keyword args collapse to
   one object; a mix keeps the array form with each kv a single-key object. *)
let bag_arg_forms () =
  let src =
    {|struct s { a: i64 @flag(verbose) @cfg(mode: fast) @scale(2.5)
       @tune(rate: 0.5) @http(method: "get", path: "/x") @mix(first, k: 1) }|}
  in
  let a = member "a" src in
  let value id =
    let t = List.find (fun (t : Ir.trait) -> t.trait_id = id) a.traits in
    Ir_json.to_canonical_string t.value
  in
  Alcotest.(check string) "positional arg" {|["verbose"]|} (value "flag");
  Alcotest.(check string) "single kv" {|{"mode":"fast"}|} (value "cfg");
  Alcotest.(check string) "positional float arg" {|[2.5]|} (value "scale");
  Alcotest.(check string) "kv float arg" {|{"rate":0.5}|} (value "tune");
  Alcotest.(check string)
    "multi kv merged" {|{"method":"get","path":"/x"}|} (value "http");
  Alcotest.(check string)
    "mixed stays array" {|["first",{"k":1}]|} (value "mix")

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

let msg_contains sub ds =
  let has s =
    let n = String.length sub and m = String.length s in
    let rec go i = i + n <= m && (String.sub s i n = sub || go (i + 1)) in
    n = 0 || go 0
  in
  List.exists (fun (d : Diagnostic.t) -> has d.message) ds

(* An unrecognized bound shape is flagged rather than silently dropped. *)
let unparsed_bounds_diagnosed () =
  Alcotest.(check bool)
    "single positional range flagged" true
    (msg_contains "@range expects" (diags_of "struct s { a: i64 @range(5) }"));
  Alcotest.(check bool)
    "single positional length flagged" true
    (msg_contains "@length expects"
       (diags_of "struct s { a: string @length(5) }"));
  Alcotest.(check bool)
    "mistyped kv bound flagged" true
    (msg_contains "@range expects"
       (diags_of {|struct s { a: i64 @range(min: "x") }|}));
  (* A partial mistyped bound (one good, one bad) is still flagged per-arg. *)
  Alcotest.(check bool)
    "partial mistyped kv bound flagged" true
    (msg_contains "max must be a number"
       (diags_of {|struct s { a: i64 @range(min: 5, max: "x") }|}))

(* Lowering records the requested [required] state and leaves the
   nullable-vs-required contradiction for the typechecker (TC0007), so it emits
   no diagnostic of its own here. *)
let required_on_nullable () =
  Alcotest.(check int)
    "lowering does not judge the conflict" 0
    (List.length (diags_of "struct s { a: string? @required }"));
  Alcotest.(check bool)
    "required flag is recorded" true
    (member "a" "struct s { a: string? @required }").required;
  Alcotest.(check int)
    "required on a plain member is clean" 0
    (List.length (diags_of "struct s { a: string @required }"))

(* Signed and fractional numbers flow through to constraints and defaults. *)
let signed_and_fractional () =
  let a = member "a" "struct s { a: i64 @range(min: -10, max: 10) }" in
  Alcotest.(check string)
    "negative keyword bound"
    {|{"range":{"exclMax":false,"exclMin":false,"max":10.0,"min":-10.0}}|}
    (cons_str (List.hd a.constraints));
  let b = member "b" "struct s { b: float @range(-1.5, 1.5) }" in
  Alcotest.(check string)
    "fractional positional bound"
    {|{"range":{"exclMax":false,"exclMin":false,"max":1.5,"min":-1.5}}|}
    (cons_str (List.hd b.constraints));
  let c = member "c" "struct s { c: float @multipleOf(0.5) }" in
  Alcotest.(check string)
    "fractional multipleOf" {|{"multipleOf":0.5}|}
    (cons_str (List.hd c.constraints));
  let d = member "d" "struct s { d: i64 @default(-1) }" in
  Alcotest.(check string)
    "negative default" "-1"
    (match d.default with
    | Some v -> Ir_json.to_canonical_string v
    | None -> "<none>")

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
    (List.exists (fun (t : Ir.trait) -> t.trait_id = "pub") s.traits)

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
          Alcotest.test_case "unparsed bounds diagnosed" `Quick
            unparsed_bounds_diagnosed;
          Alcotest.test_case "required on nullable" `Quick required_on_nullable;
          Alcotest.test_case "signed and fractional" `Quick
            signed_and_fractional;
          Alcotest.test_case "generic param use" `Quick generic_param_use;
          Alcotest.test_case "pub trait" `Quick pub_trait;
          Alcotest.test_case "snake_case checks" `Quick snake_case_checks;
        ] );
    ]
