open Tono_frontend

(* The parser never raises; on malformed input it records diagnostics and returns
   a best-effort AST so later passes can keep going. These tests pin the recovery
   behaviour at each entry point. *)

let state src =
  let toks, ld = Lexer.tokenize src in
  (Parser_state.create toks, ld)

let type_diags src =
  let st, ld = state src in
  let _ = Parser.parse_type st in
  ld @ Parser_state.diagnostics st

let trait_diags src =
  let st, ld = state src in
  let _ = Parser.parse_trait st in
  ld @ Parser_state.diagnostics st

let member_diags src =
  let st, ld = state src in
  let _ = Parser.parse_member st in
  ld @ Parser_state.diagnostics st

let struct_diags src =
  let st, ld = state src in
  let _ = Parser.parse_struct st ~pub:false ~dtraits:[] in
  ld @ Parser_state.diagnostics st

let nonempty name ds = Alcotest.(check bool) name true (List.length ds >= 1)

(* ── Types ─────────────────────────────────────────────────────────────── *)

let non_type_token () =
  let st, _ = state "@" in
  let ast = Parser.parse_type st in
  (match ast with
  | Ast.TError _ -> ()
  | _ -> Alcotest.fail "expected TError for a non-type token");
  nonempty "diagnosed" (Parser_state.diagnostics st)

let list_missing_bracket () = nonempty "missing ']'" (type_diags "[foo")

let list_bad_element () =
  (* Brackets close but the element fails: exercises the error element span. *)
  nonempty "bad list element" (type_diags "[]@")

let map_missing_bracket () =
  nonempty "map missing ']'" (type_diags "map[string")

let unclosed_generic () = nonempty "unclosed generic" (type_diags "Page[Charge")

(* ── Traits ────────────────────────────────────────────────────────────── *)

let trait_name_missing () = nonempty "no name after '@'" (trait_diags "@5")

let trait_value_missing () =
  nonempty "no value after ':'" (trait_diags "@range(min:)")

let trait_arg_invalid () =
  nonempty "bad trait argument" (trait_diags "@range(,)")

let empty_trait_args () =
  let st, _ = state "@flag()" in
  let tr = Parser.parse_trait st in
  Alcotest.(check int) "empty arg list" 0 (List.length tr.targs);
  Alcotest.(check int)
    "no diagnostics" 0
    (List.length (Parser_state.diagnostics st))

(* A primitive keyword is accepted as a trait name at parse time (lowering, not
   the parser, decides what trait names mean). *)
let trait_named_like_prim () =
  let st, _ = state "@uuid" in
  let tr = Parser.parse_trait st in
  Alcotest.(check string) "name from prim keyword" "uuid" tr.tname;
  Alcotest.(check int)
    "no diagnostics" 0
    (List.length (Parser_state.diagnostics st))

(* ── Members ───────────────────────────────────────────────────────────── *)

let member_name_missing () = nonempty "no member name" (member_diags ":i64")

let member_colon_missing () =
  nonempty "no ':' after name" (member_diags "x i64")

(* ── Structs ───────────────────────────────────────────────────────────── *)

let struct_name_missing () =
  nonempty "no struct name" (struct_diags "struct { a: i64 }")

let struct_braces_missing () = nonempty "no braces" (struct_diags "struct s")

let struct_body_recovery () =
  (* Stray ',' is skipped and a stray '@' is reported, then parsing resumes and
     still finds the well-formed member. *)
  let src = "struct s { , @ a: i64 }" in
  nonempty "stray token reported" (struct_diags src);
  let st, _ = state src in
  let decl = Parser.parse_struct st ~pub:false ~dtraits:[] in
  match decl.dkind with
  | Ast.DStruct { members; _ } ->
      Alcotest.(check (list string))
        "recovered member" [ "a" ]
        (List.map (fun (m : Ast.member) -> m.mname) members)
  | _ -> Alcotest.fail "expected a struct"

let generics_errors () =
  nonempty "bad type parameter" (struct_diags "struct s[1] { a: i64 }");
  nonempty "trailing comma in params" (struct_diags "struct p[T,] { a: i64 }")

(* ── enum / union / op ─────────────────────────────────────────────────── *)

let decl_diags parse src =
  let st, ld = state src in
  let _ = parse st ~pub:false ~dtraits:[] in
  ld @ Parser_state.diagnostics st

let enum_errors () =
  nonempty "missing enum name" (decl_diags Parser.parse_enum "enum { a }");
  nonempty "missing brace" (decl_diags Parser.parse_enum "enum e a }");
  nonempty "non-int after '='" (decl_diags Parser.parse_enum "enum e { a = x }");
  nonempty "junk in body" (decl_diags Parser.parse_enum "enum e { a : i64 }")

let contains ~sub s =
  let n = String.length sub and m = String.length s in
  let rec go i = i + n <= m && (String.sub s i n = sub || go (i + 1)) in
  n = 0 || go 0

let union_errors () =
  nonempty "missing union name" (decl_diags Parser.parse_union "union { a: a }");
  (* A stray token in a union body names "union", not "struct". *)
  let ds = decl_diags Parser.parse_union "union u { ? }" in
  Alcotest.(check bool)
    "body diagnostic names union" true
    (List.exists
       (fun (d : Diagnostic.t) -> contains ~sub:"union body" d.message)
       ds)

let op_errors () =
  nonempty "missing op name" (decl_diags Parser.parse_op "op () -> charge");
  nonempty "missing paren" (decl_diags Parser.parse_op "op create -> charge")

(* Diagnostics point at the offending token, not the start of input. *)
let diagnostic_span_and_message () =
  let st, _ = state "struct s { x i64 }" in
  let _ = Parser.parse_struct st ~pub:false ~dtraits:[] in
  match Parser_state.diagnostics st with
  | d :: _ ->
      Alcotest.(check bool) "mentions ':'" true (contains ~sub:"':'" d.message);
      (* 'i64' begins at offset 13 / column 14, where the ':' was expected. *)
      Alcotest.(check int) "span offset at i64" 13 d.span.start.offset;
      Alcotest.(check int) "span column at i64" 14 d.span.start.col
  | [] -> Alcotest.fail "expected a diagnostic"

(* ── Well-formed repetition (fills the comma loops) ─────────────────────── *)

let repetition_paths () =
  Alcotest.(check int)
    "2-arg generic clean" 0
    (List.length (type_diags "Pair[string, i64]"));
  Alcotest.(check int)
    "2-param struct clean" 0
    (List.length (struct_diags "struct p[A, B] { a: i64 }"))

let () =
  Alcotest.run "parse-error"
    [
      ( "types",
        [
          Alcotest.test_case "non-type token" `Quick non_type_token;
          Alcotest.test_case "list missing bracket" `Quick list_missing_bracket;
          Alcotest.test_case "list bad element" `Quick list_bad_element;
          Alcotest.test_case "map missing bracket" `Quick map_missing_bracket;
          Alcotest.test_case "unclosed generic" `Quick unclosed_generic;
        ] );
      ( "traits",
        [
          Alcotest.test_case "name missing" `Quick trait_name_missing;
          Alcotest.test_case "value missing" `Quick trait_value_missing;
          Alcotest.test_case "arg invalid" `Quick trait_arg_invalid;
          Alcotest.test_case "empty args" `Quick empty_trait_args;
          Alcotest.test_case "named like prim" `Quick trait_named_like_prim;
        ] );
      ( "members",
        [
          Alcotest.test_case "name missing" `Quick member_name_missing;
          Alcotest.test_case "colon missing" `Quick member_colon_missing;
        ] );
      ( "structs",
        [
          Alcotest.test_case "name missing" `Quick struct_name_missing;
          Alcotest.test_case "braces missing" `Quick struct_braces_missing;
          Alcotest.test_case "body recovery" `Quick struct_body_recovery;
          Alcotest.test_case "generics errors" `Quick generics_errors;
          Alcotest.test_case "repetition paths" `Quick repetition_paths;
        ] );
      ( "decls",
        [
          Alcotest.test_case "enum errors" `Quick enum_errors;
          Alcotest.test_case "union errors" `Quick union_errors;
          Alcotest.test_case "op errors" `Quick op_errors;
          Alcotest.test_case "diagnostic span and message" `Quick
            diagnostic_span_and_message;
        ] );
    ]
