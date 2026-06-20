open Tono_frontend

(* ── Spans ─────────────────────────────────────────────────────────────── *)

let pos line col offset : Span.pos = { line; col; offset }
let sp s f : Span.span = { start = s; finish = f }

let span_single_line () =
  Alcotest.(check string)
    "same line renders col-col" "1:1-5"
    (Span.to_string (sp (pos 1 1 0) (pos 1 5 4)))

let span_multi_line () =
  Alcotest.(check string)
    "different lines render line:col-line:col" "1:3-2:4"
    (Span.to_string (sp (pos 1 3 2) (pos 2 4 9)))

let span_merge () =
  let merged =
    Span.merge (sp (pos 1 1 0) (pos 1 2 1)) (sp (pos 3 1 5) (pos 3 4 8))
  in
  Alcotest.(check string)
    "merge takes start of a, finish of b" "1:1-3:4" (Span.to_string merged)

(* ── Diagnostics ───────────────────────────────────────────────────────── *)

let some_span = sp (pos 1 1 0) (pos 1 5 4)

let severity_strings () =
  Alcotest.(check string)
    "error" "error"
    (Diagnostic.severity_to_string Diagnostic.Error);
  Alcotest.(check string)
    "warning" "warning"
    (Diagnostic.severity_to_string Diagnostic.Warning)

let error_and_warning () =
  let e = Diagnostic.error some_span "boom" in
  let w = Diagnostic.warning some_span "careful" in
  Alcotest.(check bool) "error severity" true (e.severity = Diagnostic.Error);
  Alcotest.(check bool) "warning severity" true (w.severity = Diagnostic.Warning)

let diag_to_string () =
  Alcotest.(check string)
    "error renders" "1:1-5: error: boom"
    (Diagnostic.to_string (Diagnostic.error some_span "boom"));
  Alcotest.(check string)
    "warning renders" "1:1-5: warning: careful"
    (Diagnostic.to_string (Diagnostic.warning some_span "careful"))

(* ── Token descriptions ────────────────────────────────────────────────── *)

let describe_all () =
  let all : Token.kind list =
    [
      KwStruct;
      KwEnum;
      KwUnion;
      KwOp;
      KwMap;
      KwPub;
      KwThrows;
      Ident "x";
      Prim "i64";
      Str "s";
      Int 5;
      At;
      LBrace;
      RBrace;
      LBracket;
      RBracket;
      LParen;
      RParen;
      Colon;
      Question;
      Comma;
      Dot;
      Eq;
      Arrow;
      Eof;
    ]
  in
  Alcotest.(check (list string))
    "every kind describes itself"
    [
      "'struct'";
      "'enum'";
      "'union'";
      "'op'";
      "'map'";
      "'pub'";
      "'throws'";
      "identifier 'x'";
      "type 'i64'";
      "string literal";
      "integer '5'";
      "'@'";
      "'{'";
      "'}'";
      "'['";
      "']'";
      "'('";
      "')'";
      "':'";
      "'?'";
      "','";
      "'.'";
      "'='";
      "'->'";
      "end of file";
    ]
    (List.map Token.describe all)

(* ── AST spans ─────────────────────────────────────────────────────────── *)

let ty_span_all () =
  let s = sp (pos 1 1 0) (pos 1 2 1) in
  let cases : Ast.ty list =
    [
      TPrim ("i64", s);
      TName ("X", [], s);
      TList (TPrim ("i64", s), s);
      TMap (TPrim ("string", s), TPrim ("i64", s), s);
      TNullable (TPrim ("i64", s), s);
      TError s;
    ]
  in
  List.iter
    (fun t ->
      Alcotest.(check bool) "span passes through" true (Ast.ty_span t = s))
    cases

(* ── Parser cursor ─────────────────────────────────────────────────────── *)

let tok_only src = fst (Lexer.tokenize src)

let cursor_advance_stops_at_eof () =
  let st = Parser_state.create (tok_only "") in
  let a = Parser_state.advance st in
  let b = Parser_state.advance st in
  Alcotest.(check bool)
    "advance never moves past eof" true
    (a.kind = Token.Eof && b.kind = Token.Eof)

let cursor_expect () =
  let st = Parser_state.create (tok_only "{") in
  (match Parser_state.expect st Token.LBrace "'{'" with
  | Some t ->
      Alcotest.(check bool) "consumes a match" true (t.kind = Token.LBrace)
  | None -> Alcotest.fail "expected a match");
  (match Parser_state.expect st Token.RBrace "'}'" with
  | None -> ()
  | Some _ -> Alcotest.fail "should not match at eof");
  Alcotest.(check bool)
    "mismatch diagnosed" true
    (List.length (Parser_state.diagnostics st) >= 1)

let () =
  Alcotest.run "core"
    [
      ( "span",
        [
          Alcotest.test_case "single line" `Quick span_single_line;
          Alcotest.test_case "multi line" `Quick span_multi_line;
          Alcotest.test_case "merge" `Quick span_merge;
        ] );
      ( "diagnostic",
        [
          Alcotest.test_case "severity strings" `Quick severity_strings;
          Alcotest.test_case "error and warning" `Quick error_and_warning;
          Alcotest.test_case "to_string" `Quick diag_to_string;
        ] );
      ("token", [ Alcotest.test_case "describe all" `Quick describe_all ]);
      ("ast", [ Alcotest.test_case "ty_span all arms" `Quick ty_span_all ]);
      ( "cursor",
        [
          Alcotest.test_case "advance stops at eof" `Quick
            cursor_advance_stops_at_eof;
          Alcotest.test_case "expect" `Quick cursor_expect;
        ] );
    ]
