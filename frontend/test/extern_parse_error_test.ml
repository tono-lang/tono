open Tono_frontend

(* Malformed-input regression tests for the "ext <name> { ... }" FFI library
   block (Parser_extern), following the recovery-behaviour convention set by
   parse_error_test.ml. The parser never raises; every case below is expected
   to record at least one diagnostic and still return a best-effort AST. *)

let file_diags src = snd (Parser.parse src)
let nonempty name ds = Alcotest.(check bool) name true (List.length ds >= 1)

let missing_call_line () =
  nonempty "language block without 'call:'"
    (file_diags
       {|ext mylib {
         go: "example.com/mylib"
         extern load(): string {
           go { yields: (x: string) }
         }
       }|})

let empty_yields () =
  nonempty "empty 'yields: ()'"
    (file_diags
       {|ext mylib {
         go: "example.com/mylib"
         extern load(): string {
           go { call: "Load"() yields: () }
         }
       }|})

let returns_missing_type () =
  nonempty "'returns:' with no type before '{'"
    (file_diags
       {|ext mylib {
         go: "example.com/mylib"
         extern load(): string {
           go { call: "Load"() returns: { } }
         }
       }|})

let error_as_extern_return_type () =
  nonempty "'error' used as an extern return type"
    (file_diags
       {|ext mylib {
         go: "example.com/mylib"
         extern load(): error {
           go { call: "Load"() }
         }
       }|})

let error_as_returns_type () =
  nonempty "'error' used as a 'returns:' type"
    (file_diags
       {|ext mylib {
         go: "example.com/mylib"
         extern load(): string {
           go { call: "Load"() returns: error { } }
         }
       }|})

(* Outside 'yields:'/'returns:'/an extern's own return type, 'error' is an
   ordinary identifier: a foreign struct may still name a field 'error'. *)
let error_as_ordinary_field_name () =
  let ds =
    file_diags
      {|ext mylib {
        go: "example.com/mylib"
        struct go_result { error: string }
      }|}
  in
  Alcotest.(check int) "no diagnostics" 0 (List.length ds)

(* At 'ext <ident>', the old-grammar kind words (hook/contract/constraint/
   impl) dispatch to the legacy Parser_ext; any other identifier dispatches
   to the new library-block grammar. *)
let kind_dispatch_disambiguation () =
  let st, _ =
    let toks, ld = Lexer.tokenize {|ext hook before_request { ts: "a#b" }|} in
    (Parser_state.create toks, ld)
  in
  let decl = Parser.parse_decl st in
  (match decl with
  | Some { dkind = Ast.DExt _; _ } -> ()
  | _ -> Alcotest.fail "expected the old-grammar 'ext hook' to parse as DExt");
  let st2, _ =
    let toks, ld = Lexer.tokenize {|ext mylib { go: "example.com/mylib" }|} in
    (Parser_state.create toks, ld)
  in
  let decl2 = Parser.parse_decl st2 in
  match decl2 with
  | Some { dkind = Ast.DExtLib _; _ } -> ()
  | _ -> Alcotest.fail "expected 'ext mylib' to parse as DExtLib"

let () =
  Alcotest.run "extern-parse-error"
    [
      ( "language block",
        [
          Alcotest.test_case "missing call line" `Quick missing_call_line;
          Alcotest.test_case "empty yields" `Quick empty_yields;
          Alcotest.test_case "returns missing type" `Quick returns_missing_type;
        ] );
      ( "'error' reserved word",
        [
          Alcotest.test_case "as extern return type" `Quick
            error_as_extern_return_type;
          Alcotest.test_case "as returns type" `Quick error_as_returns_type;
          Alcotest.test_case "as ordinary field name" `Quick
            error_as_ordinary_field_name;
        ] );
      ( "kind dispatch",
        [
          Alcotest.test_case "hook vs library name" `Quick
            kind_dispatch_disambiguation;
        ] );
    ]
