open Tono_frontend

let lower_ty ?(params = []) src =
  let toks, ld = Lexer.tokenize src in
  let st = Parser_state.create toks in
  let ast = Parser.parse_type st in
  let diags = ref [] in
  let tref = Lower.lower_type ~params ~diags ast in
  (ast, tref, ld @ Parser_state.diagnostics st @ List.rev !diags)

let show ?(params = []) src =
  let _, tref, _ = lower_ty ~params src in
  Ir_json.to_canonical_string (Ir_json.encode_tref tref)

let diags ?(params = []) src =
  let _, _, ds = lower_ty ~params src in
  ds

let check name expected src = Alcotest.(check string) name expected (show src)

let primitives () =
  check "i64" {|{"prim":"i64"}|} "i64";
  check "u32" {|{"prim":"u32"}|} "u32";
  check "string" {|{"prim":"string"}|} "string";
  check "timestamp" {|{"prim":"timestamp"}|} "timestamp";
  check "uuid" {|{"prim":"uuid"}|} "uuid"

let decimal_rejected () =
  Alcotest.(check bool)
    "decimal diagnosed" true
    (List.length (diags "decimal") >= 1)

let refs_and_generics () =
  check "named ref" {|{"args":[],"ref":"Charge"}|} "Charge";
  check "generic app" {|{"args":[{"args":[],"ref":"Charge"}],"ref":"Page"}|}
    "Page[Charge]";
  check "nested generic"
    {|{"args":[{"map":[{"prim":"string"},{"args":[],"ref":"Charge"}]}],"ref":"Page"}|}
    "Page[map[string]Charge]"

let list_and_map () =
  check "list" {|{"list":{"args":[],"ref":"Charge"}}|} "[]Charge";
  check "list of prim" {|{"list":{"prim":"string"}}|} "[]string";
  check "map" {|{"map":[{"prim":"string"},{"args":[],"ref":"Charge"}]}|}
    "map[string]Charge"

let params () =
  Alcotest.(check string)
    "param use" {|{"param":"T"}|} (show ~params:[ "T" ] "T");
  Alcotest.(check string)
    "list of param" {|{"list":{"param":"T"}}|}
    (show ~params:[ "T" ] "[]T");
  (* Without the param in scope it is an ordinary named reference. *)
  Alcotest.(check string)
    "ref when not a param" {|{"args":[],"ref":"T"}|} (show "T")

let nullable_parses () =
  let ast, _, ds = lower_ty "string?" in
  Alcotest.(check int) "no diagnostics" 0 (List.length ds);
  match ast with
  | Ast.TNullable (Ast.TPrim ("string", _), _) -> ()
  | _ -> Alcotest.fail "expected a nullable string"

(* Pins the keyword-to-IR-primitive table, including the defensive default that
   the lexer's primitive set never actually triggers. *)
let prim_keyword_map () =
  let p = Lower.Internal.prim_of_keyword in
  let eq name expected got = Alcotest.(check bool) name true (expected = got) in
  eq "bool" Ir.Bool (p "bool");
  eq "string" Ir.String (p "string");
  eq "bytes" Ir.Bytes (p "bytes");
  eq "float" Ir.Float (p "float");
  eq "timestamp" Ir.Timestamp (p "timestamp");
  eq "date" Ir.Date (p "date");
  eq "duration" Ir.Duration (p "duration");
  eq "uuid" Ir.Uuid (p "uuid");
  eq "i8" (Ir.int_prim ~bits:8 ~signed:true) (p "i8");
  eq "i16" (Ir.int_prim ~bits:16 ~signed:true) (p "i16");
  eq "i32" (Ir.int_prim ~bits:32 ~signed:true) (p "i32");
  eq "i64" (Ir.int_prim ~bits:64 ~signed:true) (p "i64");
  eq "u8" (Ir.int_prim ~bits:8 ~signed:false) (p "u8");
  eq "u16" (Ir.int_prim ~bits:16 ~signed:false) (p "u16");
  eq "u32" (Ir.int_prim ~bits:32 ~signed:false) (p "u32");
  eq "u64" (Ir.int_prim ~bits:64 ~signed:false) (p "u64");
  eq "unknown falls back to string" Ir.String (p "nope")

(* A type position that fails to parse lowers to an empty named reference. *)
let error_type_lowers () =
  let toks, _ = Lexer.tokenize ":" in
  let st = Parser_state.create toks in
  let ast = Parser.parse_type st in
  (match ast with
  | Ast.TError _ -> ()
  | _ -> Alcotest.fail "expected TError from a non-type token");
  let diags = ref [] in
  let tref = Lower.lower_type ~params:[] ~diags ast in
  Alcotest.(check string)
    "error type lowers to empty ref" {|{"args":[],"ref":""}|}
    (Ir_json.to_canonical_string (Ir_json.encode_tref tref))

let () =
  Alcotest.run "type"
    [
      ( "lower",
        [
          Alcotest.test_case "primitives" `Quick primitives;
          Alcotest.test_case "decimal rejected" `Quick decimal_rejected;
          Alcotest.test_case "refs and generics" `Quick refs_and_generics;
          Alcotest.test_case "list and map" `Quick list_and_map;
          Alcotest.test_case "params" `Quick params;
          Alcotest.test_case "nullable parses" `Quick nullable_parses;
          Alcotest.test_case "prim keyword map" `Quick prim_keyword_map;
          Alcotest.test_case "error type lowers" `Quick error_type_lowers;
        ] );
    ]
