open Tono_frontend
module A = Calc_ast

(* A compact s-expression rendering of an expression, for structural assertions. *)
let binop = function
  | A.Add -> "+"
  | A.Sub -> "-"
  | A.Mul -> "*"
  | A.Div -> "/"
  | A.Mod -> "%"
  | A.Eq -> "=="
  | A.Ne -> "!="
  | A.Lt -> "<"
  | A.Gt -> ">"
  | A.Le -> "<="
  | A.Ge -> ">="
  | A.And -> "&&"
  | A.Or -> "||"
  | A.Concat -> "++"

let rec show (e : A.expr) =
  match e.kind with
  | A.Lit (A.Int s) -> s
  | A.Lit (A.Float f) -> Printf.sprintf "%g" f
  | A.Lit (A.Str s) -> Printf.sprintf "%S" s
  | A.Lit (A.Bool b) -> string_of_bool b
  | A.Var v -> v
  | A.Field (e, f) -> Printf.sprintf "%s.%s" (show e) f
  | A.Not e -> Printf.sprintf "!%s" (show e)
  | A.Binop (op, a, b) ->
      Printf.sprintf "(%s %s %s)" (show a) (binop op) (show b)
  | A.If (c, t, e) -> Printf.sprintf "if(%s,%s,%s)" (show c) (show t) (show e)
  | A.Let (x, v, b) -> Printf.sprintf "let(%s=%s,%s)" x (show v) (show b)
  | A.Call (f, args) ->
      Printf.sprintf "%s(%s)" f (String.concat "," (List.map show args))
  | A.Map (e, f) -> Printf.sprintf "map(%s,%s)" (show e) f
  | A.Filter (e, f) -> Printf.sprintf "filter(%s,%s)" (show e) f
  | A.Fold (e, i, f) -> Printf.sprintf "fold(%s,%s,%s)" (show e) (show i) f
  | A.Match (s, arms) ->
      Printf.sprintf "match(%s){%s}" (show s)
        (String.concat ";"
           (List.map (fun (p, b) -> pat p ^ "=>" ^ show b) arms))
  | A.Some_ e -> Printf.sprintf "Some(%s)" (show e)
  | A.None_ -> "None"
  | A.Coalesce (a, b) -> Printf.sprintf "(%s??%s)" (show a) (show b)
  | A.Ctor (n, fs) ->
      Printf.sprintf "%s{%s}" n
        (String.concat "," (List.map (fun (f, e) -> f ^ ":" ^ show e) fs))
  | A.EError -> "<err>"

and pat = function
  | A.PVariant { variant; bind = Some b } -> Printf.sprintf "%s(%s)" variant b
  | A.PVariant { variant; bind = None } -> variant
  | A.PUnknown { bind } -> Printf.sprintf "Unknown(%s)" bind

(* Parse a single-expression function and render its body. *)
let body src =
  match Calc_parser.parse ("fn f() -> i64 = " ^ src) with
  | [ fd ], _ -> show fd.A.body
  | _ -> "<no-fn>"

let renders label expected src =
  Alcotest.(check string) label expected (body src)

(* Diagnostics count for a whole program. *)
let ndiags src = List.length (snd (Calc_parser.parse src))

(* ── Atoms and literals ────────────────────────────────────────────────── *)

let atoms () =
  renders "int" "7" "7";
  renders "neg int" "-7" "-7";
  renders "float" "1.5" "1.5";
  renders "neg float" "-1.5" "-1.5";
  renders "string" "\"hi\"" "\"hi\"";
  renders "true" "true" "true";
  renders "false" "false" "false";
  renders "var" "x" "x";
  renders "field" "a.b.c" "a.b.c"

(* ── Precedence and associativity ──────────────────────────────────────── *)

let precedence () =
  renders "mul before add" "(1 + (2 * 3))" "1 + 2 * 3";
  renders "add left-assoc" "((1 - 2) - 3)" "1 - 2 - 3";
  renders "concat at add level" "(\"a\" ++ \"b\")" "\"a\" ++ \"b\"";
  renders "cmp below add" "((1 + 2) == 3)" "1 + 2 == 3";
  renders "and below cmp" "((a == 0) && b)" "a == 0 && b";
  renders "or below and" "((a && b) || c)" "a && b || c";
  renders "coalesce outermost right-assoc" "(a??(b??c))" "a ?? b ?? c";
  renders "not binds tight" "(!a && b)" "!a && b";
  renders "paren overrides" "((1 + 2) * 3)" "(1 + 2) * 3"

(* ── Compound forms ────────────────────────────────────────────────────── *)

let forms () =
  renders "if" "if(x,1,2)" "if x then 1 else 2";
  renders "let" "let(y=1,(y + 2))" "let y = 1 in y + 2";
  renders "some" "Some(x)" "Some(x)";
  renders "none" "None" "None";
  renders "coalesce" "(x??0)" "x ?? 0";
  renders "call with args" "g(a,b)" "g(a, b)";
  renders "call no args" "g()" "g()";
  renders "map" "map(xs,f)" "map(xs, f)";
  renders "filter" "filter(xs,f)" "filter(xs, f)";
  renders "fold" "fold(xs,0,f)" "fold(xs, 0, f)";
  renders "ctor" "p{x:1,y:2}" "p { x: 1, y: 2 }";
  renders "ctor empty" "p{}" "p {  }"

let matches () =
  renders "variant with bind" "match(p){c(x)=>1;Unknown(u)=>2}"
    "match p { c(x) => 1 Unknown(u) => 2 }";
  renders "variant no bind" "match(p){a=>1;Unknown(u)=>0}"
    "match p { a => 1 Unknown(u) => 0 }";
  (* A ctor scrutinee needs parentheses; a call scrutinee does not. *)
  renders "call scrutinee" "match(get(xs,0)){a=>1;Unknown(u)=>0}"
    "match get(xs, 0) { a => 1 Unknown(u) => 0 }"

(* ── Boundary types parse (no diagnostics) ─────────────────────────────── *)

let types_parse () =
  let ok t =
    Alcotest.(check int) t 0 (ndiags ("fn f(x: " ^ t ^ ") -> i64 = 1"))
  in
  ok "i64";
  ok "string";
  ok "item";
  ok "[]i64";
  ok "map[string]item";
  ok "item?";
  ok "[]item?";
  Alcotest.(check int)
    "nested map list" 0
    (ndiags "fn f(x: map[string][]i64) -> i64 = 1")

let clean_program () =
  Alcotest.(check int)
    "multi-fn program is clean" 0
    (ndiags
       "fn add(a: i64, b: i64) -> i64 = a + b\n\
        fn pick(p: pm) -> string = match p { card(c) => c.last4 Unknown(u) => \
        u }")

(* ── Negatives: each is outside the grammar or malformed ───────────────── *)

let negatives () =
  let bad label src = Alcotest.(check bool) label true (ndiags src >= 1) in
  bad "missing body" "fn f() -> i64 =";
  bad "missing arrow" "fn f() = 1";
  bad "missing else" "fn f() -> i64 = if x then 1";
  bad "dangling operator" "fn f() -> i64 = 1 +";
  bad "minus without literal" "fn f() -> i64 = - x";
  bad "map missing fn arg" "fn f() -> i64 = map(xs)";
  bad "fold missing init" "fn f() -> i64 = fold(xs, g)";
  bad "lambda is not in the grammar" "fn f() -> i64 = \\x -> x";
  bad "loop keyword leftovers" "fn f() -> i64 = while true 1";
  bad "throw is not in the grammar" "fn f() -> i64 = throw 1 2";
  bad "bad type" "fn f(x: +) -> i64 = 1";
  bad "bad pattern" "fn f() -> i64 = match p { 1 => 2 }";
  bad "unclosed match runs to eof" "fn f() -> i64 = match p { a => 1";
  bad "stray brace" "}";
  bad "unexpected char" "fn f() -> i64 = #"

(* ── Lexer ─────────────────────────────────────────────────────────────── *)

let lexer_ops () =
  let toks, diags =
    Calc_lexer.tokenize
      "+ - * / % == != < > <= >= && || ! ++ ?? -> => = ( ) { } [ ] : , . ?"
  in
  Alcotest.(check int) "no lex diags" 0 (List.length diags);
  (* every operator/punctuation token, plus the trailing Eof *)
  Alcotest.(check int) "operator token count" 30 (List.length toks)

let lexer_keywords () =
  let toks, diags =
    Calc_lexer.tokenize "fn if then else let in match map filter fold Some None"
  in
  Alcotest.(check int) "no diags" 0 (List.length diags);
  Alcotest.(check int) "keyword count" 13 (List.length toks)

let lexer_trivia () =
  let toks, diags = Calc_lexer.tokenize "// comment\n  42 // trailing" in
  Alcotest.(check int) "comment skipped, clean" 0 (List.length diags);
  Alcotest.(check int) "one value token plus eof" 2 (List.length toks)

let lexer_strings () =
  let toks, diags = Calc_lexer.tokenize {|"a\nb\t\"c\\d\re"|} in
  Alcotest.(check int) "escapes clean" 0 (List.length diags);
  match toks with
  | [ { kind = Calc_token.Str s; _ }; _ ] ->
      Alcotest.(check string) "decoded" "a\nb\t\"c\\d\re" s
  | _ -> Alcotest.fail "expected one string token"

let lexer_errors () =
  let bad label s =
    Alcotest.(check bool)
      label true
      (List.length (snd (Calc_lexer.tokenize s)) >= 1)
  in
  bad "lone amp" "&";
  bad "lone pipe" "|";
  bad "unterminated" {|"oops|};
  bad "newline in string" "\"oops\n\"";
  bad "invalid escape" {|"a\qb"|};
  bad "backslash at eof" "\"a\\";
  bad "unexpected char" "#"

(* Tab/carriage-return whitespace, an ident starting with '_', and a trailing
   dot with no fractional digits. *)
let lexer_edges () =
  let clean label src n =
    let toks, diags = Calc_lexer.tokenize src in
    Alcotest.(check int) (label ^ " diags") 0 (List.length diags);
    Alcotest.(check int) (label ^ " count") n (List.length toks)
  in
  clean "tab/cr ws and _ident" "1\t_x\r2" 4;
  clean "trailing dot, no fraction" "1." 3;
  clean "dot then non-digit" "1.x" 4

(* Exercise every token's [describe] so the catalogue stays exhaustive. *)
let describe_all () =
  let open Calc_token in
  let all =
    [
      KwFn;
      KwIf;
      KwThen;
      KwElse;
      KwLet;
      KwIn;
      KwMatch;
      KwMap;
      KwFilter;
      KwFold;
      KwTrue;
      KwFalse;
      KwSome;
      KwNone;
      KwUnknown;
      Prim "i64";
      Ident "x";
      Int "1";
      Float 1.0;
      Str "s";
      Plus;
      Minus;
      Star;
      Slash;
      Percent;
      EqEq;
      BangEq;
      Lt;
      Gt;
      LtEq;
      GtEq;
      AmpAmp;
      PipePipe;
      Bang;
      PlusPlus;
      QQuestion;
      Arrow;
      FatArrow;
      Eq;
      LParen;
      RParen;
      LBrace;
      RBrace;
      LBracket;
      RBracket;
      Colon;
      Comma;
      Dot;
      Question;
      Eof;
    ]
  in
  List.iter
    (fun k ->
      Alcotest.(check bool) "non-empty" true (String.length (describe k) > 0))
    all

let () =
  Alcotest.run "calculus"
    [
      ( "lexer",
        [
          Alcotest.test_case "operators" `Quick lexer_ops;
          Alcotest.test_case "keywords" `Quick lexer_keywords;
          Alcotest.test_case "trivia" `Quick lexer_trivia;
          Alcotest.test_case "strings" `Quick lexer_strings;
          Alcotest.test_case "errors" `Quick lexer_errors;
          Alcotest.test_case "edges" `Quick lexer_edges;
          Alcotest.test_case "describe" `Quick describe_all;
        ] );
      ( "parse",
        [
          Alcotest.test_case "atoms" `Quick atoms;
          Alcotest.test_case "precedence" `Quick precedence;
          Alcotest.test_case "forms" `Quick forms;
          Alcotest.test_case "matches" `Quick matches;
          Alcotest.test_case "types parse" `Quick types_parse;
          Alcotest.test_case "clean program" `Quick clean_program;
          Alcotest.test_case "negatives" `Quick negatives;
        ] );
    ]
