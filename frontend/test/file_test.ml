open Tono_frontend

let run ?(module_name = "m") src = Tono_frontend.compile ~module_name src
let shape_ids (m : Ir.module_) = List.map (fun (s : Ir.shape) -> s.id) m.shapes
let op_ids (m : Ir.module_) = List.map (fun (s : Ir.shape) -> s.id) m.operations

let contains ~sub s =
  let n = String.length sub and m = String.length s in
  let rec go i = i + n <= m && (String.sub s i n = sub || go (i + 1)) in
  n = 0 || go 0

(* A missing shape name is reported once by the parser; lowering must not add a
   second, bogus "must be snake_case" error for the empty name. *)
let missing_shape_name () =
  let _, ds = run "struct { a: i64 }" in
  Alcotest.(check bool) "missing name reported" true (List.length ds >= 1);
  Alcotest.(check bool)
    "no snake_case error for empty name" false
    (List.exists
       (fun (d : Diagnostic.t) -> contains ~sub:"snake_case" d.message)
       ds)

(* Operations land in [operations], every other shape in [shapes]. *)
let multi_decl () =
  let src =
    {|
struct charge { id: uuid, amount: i64 }
enum status { active, closed }
union source { card: card, bank: bank_account }
op list_charges() -> page[charge]
|}
  in
  let m, ds = run src in
  Alcotest.(check int) "no diagnostics" 0 (List.length ds);
  Alcotest.(check string) "module name" "m" m.mod_name;
  (* ids are qualified with the module name during resolution *)
  Alcotest.(check (list string))
    "shapes in order"
    [ "m#charge"; "m#status"; "m#source" ]
    (shape_ids m);
  Alcotest.(check (list string)) "operations" [ "m#list_charges" ] (op_ids m)

(* The shape-level pub and traits flow end to end through the file parser. *)
let pub_and_traits () =
  let m, ds = run {|@doc("a charge") pub struct charge { amount: i64 }|} in
  Alcotest.(check int) "clean" 0 (List.length ds);
  match m.shapes with
  | [ s ] ->
      let ids = List.map (fun (t : Ir.trait) -> t.trait_id) s.traits in
      Alcotest.(check (list string))
        "pub then doc" [ "core#pub"; "core#doc" ] ids
  | _ -> Alcotest.fail "expected exactly one shape"

(* A stray token between declarations is reported, then parsing resumes and
   still recovers the well-formed declarations on either side. *)
let recovers_between_decls () =
  let src = "struct ok1 { a: i64 }\n }\n struct ok2 { b: i64 }" in
  let m, ds = run src in
  Alcotest.(check bool) "stray token diagnosed" true (List.length ds >= 1);
  Alcotest.(check (list string))
    "both structs parsed" [ "m#ok1"; "m#ok2" ] (shape_ids m)

(* Runs of stray tokens exercise the skip loop, and resynchronization lands on
   each kind of declaration start (keyword, trait, or pub). *)
let recovers_runs_of_garbage () =
  let src =
    "] ] struct s { a: i64 }\n\
    \ , enum e { x }\n\
    \ ) union u { m: t }\n\
    \ : op o() -> t\n\
    \ = @doc(\"d\") pub struct s2 { b: i64 }"
  in
  let m, ds = run src in
  Alcotest.(check bool) "garbage diagnosed" true (List.length ds >= 1);
  Alcotest.(check (list string))
    "shapes recovered"
    [ "m#s"; "m#e"; "m#u"; "m#s2" ]
    (shape_ids m);
  Alcotest.(check (list string)) "operation recovered" [ "m#o" ] (op_ids m)

let resync_to_pub () =
  let m, ds = run "? pub struct paid { a: i64 }" in
  Alcotest.(check bool) "stray diagnosed" true (List.length ds >= 1);
  Alcotest.(check (list string))
    "recovered after pub" [ "m#paid" ] (shape_ids m)

let empty_file () =
  let m, ds = run "  \n // just a comment\n" in
  Alcotest.(check int) "no diagnostics" 0 (List.length ds);
  Alcotest.(check (list string)) "no shapes" [] (shape_ids m);
  Alcotest.(check (list string)) "no operations" [] (op_ids m)

let dangling_traits () =
  let _, ds = run {|@doc("x")|} in
  Alcotest.(check bool)
    "trailing trait without a declaration diagnosed" true
    (List.length ds >= 1)

let default_module_name () =
  let m, _ = Tono_frontend.compile "struct a { x: i64 }" in
  Alcotest.(check string) "default module name is empty" "" m.mod_name

let members_of (s : Ir.shape) =
  match s.kind with Ir.Structure { members; _ } -> members | _ -> []

let target_of name (s : Ir.shape) =
  let mem =
    List.find (fun (m : Ir.member) -> m.Ir.name = name) (members_of s)
  in
  Ir_json.to_canonical_string (Ir_json.encode_tref mem.target)

(* A name defined in the file is qualified with the module; any other name is a
   core builtin (core#). *)
let namespace_resolution () =
  let m, ds =
    run ~module_name:"pay"
      "struct charge { next: charge\n\
      \ list: page[charge]\n\
      \ items: []charge\n\
      \ lookup: map[string]charge }"
  in
  Alcotest.(check int) "clean" 0 (List.length ds);
  let s = List.hd m.shapes in
  Alcotest.(check string) "shape id qualified" "pay#charge" s.id;
  Alcotest.(check string)
    "local ref qualified" {|{"args":[],"ref":"pay#charge"}|}
    (target_of "next" s);
  Alcotest.(check string)
    "unknown ref is core#"
    {|{"args":[{"args":[],"ref":"pay#charge"}],"ref":"core#page"}|}
    (target_of "list" s);
  (* Names inside list and map types are qualified too. *)
  Alcotest.(check string)
    "list element qualified" {|{"list":{"args":[],"ref":"pay#charge"}}|}
    (target_of "items" s);
  Alcotest.(check string)
    "map value qualified"
    {|{"map":[{"prim":"string"},{"args":[],"ref":"pay#charge"}]}|}
    (target_of "lookup" s)

(* A custom trait (not in the core set, and with no shape of the same name)
   resolves to module#; a core trait resolves to core#. *)
let trait_namespace_resolution () =
  let m, _ = run ~module_name:"lab" {|@luhn @doc("k") struct k { y: i64 }|} in
  let k = List.hd m.shapes in
  Alcotest.(check (list string))
    "custom trait module-scoped, core trait core#" [ "lab#luhn"; "core#doc" ]
    (List.map (fun (t : Ir.trait) -> t.trait_id) k.traits)

let () =
  Alcotest.run "file"
    [
      ( "compile",
        [
          Alcotest.test_case "multiple declarations" `Quick multi_decl;
          Alcotest.test_case "pub and traits end to end" `Quick pub_and_traits;
          Alcotest.test_case "recovers between decls" `Quick
            recovers_between_decls;
          Alcotest.test_case "recovers runs of garbage" `Quick
            recovers_runs_of_garbage;
          Alcotest.test_case "resync to pub" `Quick resync_to_pub;
          Alcotest.test_case "empty file" `Quick empty_file;
          Alcotest.test_case "dangling traits" `Quick dangling_traits;
          Alcotest.test_case "default module name" `Quick default_module_name;
          Alcotest.test_case "missing shape name" `Quick missing_shape_name;
          Alcotest.test_case "namespace resolution" `Quick namespace_resolution;
          Alcotest.test_case "trait namespace resolution" `Quick
            trait_namespace_resolution;
        ] );
    ]
