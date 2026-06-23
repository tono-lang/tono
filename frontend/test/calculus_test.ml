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

(* ── Type checker ──────────────────────────────────────────────────────── *)

(* A shape environment built by compiling .tono source; the calculus checks
   against it. *)
let env =
  fst
    (compile
       "struct item { amount: i64, label: string }\n\
        struct cart { items: []item, tags: map[string]i64 }\n\
        struct card { last4: string }\n\
        struct pix { key: string }\n\
        union pm { card(card), pix(pix) }\n\
        enum colour { red, green }")

let ccodes src =
  let prog, pd = Calc_parser.parse src in
  if pd <> [] then [ "PARSE" ]
  else
    List.filter_map
      (fun (d : Diagnostic.t) -> d.code)
      (Calc_check.check env prog)

let chk label expected src =
  Alcotest.(check (list string)) label expected (ccodes src)

let check_ok () =
  chk "arith" [] "fn f(a: i64, b: i64) -> i64 = a + b - a * b";
  chk "float arith" [] "fn f(a: float, b: float) -> float = a / b";
  chk "literal adopts width" [] "fn f(a: i32) -> i32 = a + 1";
  chk "compare" [] "fn f(a: i64, b: i64) -> bool = a < b && a == b || !(a > b)";
  chk "concat" [] "fn f(a: string, b: string) -> string = a ++ b";
  chk "if let" [] "fn f(a: i64) -> i64 = let y = a in if y == y then y else 0";
  chk "field" [] "fn f(it: item) -> string = it.label";
  chk "guarded div" []
    "fn f(a: i64, b: i64) -> i64 = if b != 0 then a / b else 0";
  chk "guarded mod 0==b" []
    "fn f(a: i64, b: i64) -> i64 = if 0 == b then 0 else a % b";
  chk "literal divisor" [] "fn f(a: i64) -> i64 = a / 3";
  chk "length" [] "fn f(c: cart) -> i64 = length(c.items)";
  chk "get opt" [] "fn f(c: cart) -> item? = get(c.items, 0)";
  chk "head some none" []
    "fn f(c: cart) -> item? = if length(c.items) == 0 then None else \
     head(c.items)";
  chk "lookup get_or" []
    "fn f(c: cart, k: string) -> i64 = get_or(c.tags, k, 0)";
  chk "to_int to_float" [] "fn f(x: float) -> float = to_float(to_int(x) ?? 0)";
  chk "ctor" [] "fn f() -> card = card { last4: \"1234\" }";
  chk "match union" []
    "fn f(p: pm) -> string = match p { card(c) => c.last4 pix(x) => x.key \
     Unknown(u) => \"?\" }";
  chk "match enum" []
    "fn f(c: colour) -> i64 = match c { red => 1 green => 2 Unknown(u) => 0 }";
  chk "map filter fold" []
    "fn dbl(x: i64) -> i64 = x * 2\n\
     fn pos(x: i64) -> bool = x > 0\n\
     fn add(a: i64, x: i64) -> i64 = a + x\n\
     fn f(xs: []i64) -> i64 = fold(filter(map(xs, dbl), pos), 0, add)";
  chk "find" []
    "fn pos(x: i64) -> bool = x > 0\n\
     fn f(xs: []i64) -> i64 = find(xs, pos) ?? 0"

let check_errors () =
  chk "unbound var" [ "CA0001" ] "fn f() -> i64 = x";
  chk "unknown fn" [ "CA0001" ] "fn f() -> i64 = g(1)";
  chk "combinator fn unknown" [ "CA0001" ]
    "fn f(xs: []i64) -> []i64 = map(xs, g)";
  chk "fn wrong arity" [ "CA0002" ]
    "fn g(a: i64) -> i64 = a\nfn f() -> i64 = g(1, 2)";
  chk "builtin wrong arity" [ "CA0002" ] "fn f(c: cart) -> i64 = length(c, c)";
  chk "mixed width" [ "CA0003" ] "fn f(a: i64, b: i32) -> i64 = a + b";
  chk "int plus float" [ "CA0003" ] "fn f(a: i64, b: float) -> i64 = a + b";
  chk "non-bool cond" [ "CA0003" ] "fn f(a: i64) -> i64 = if a then 1 else 2";
  chk "non-bool and" [ "CA0003" ] "fn f(a: i64, b: bool) -> bool = a && b";
  chk "concat non-string" [ "CA0003" ]
    "fn f(a: i64, b: string) -> string = a ++ b";
  chk "incomparable" [ "CA0003" ] "fn f(a: i64, b: string) -> bool = a == b";
  chk "arg mismatch" [ "CA0003" ]
    "fn g(a: string) -> string = a\nfn f() -> string = g(1)";
  chk "not a list" [ "CA0003" ] "fn f(a: i64) -> i64 = length(a)";
  chk "not a map" [ "CA0003" ]
    "fn f(c: cart) -> i64 = get_or(c.items, \"k\", 0)";
  chk "to_float non-int" [ "CA0003" ] "fn f(s: string) -> float = to_float(s)";
  chk "coalesce mismatch" [ "CA0003" ]
    "fn f(c: cart) -> string = head(c.items) ?? 0";
  chk "coalesce non-optional" [ "CA0003" ] "fn f(a: i64) -> i64 = a ?? 0";
  chk "combinator fn shape" [ "CA0003" ]
    "fn two(a: i64, b: i64) -> i64 = a\nfn f(xs: []i64) -> []i64 = map(xs, two)";
  chk "ret mismatch" [ "CA0003" ] "fn f() -> string = 1";
  chk "field non-struct" [ "CA0004" ] "fn f(a: i64) -> i64 = a.x";
  chk "unknown field" [ "CA0004" ] "fn f(it: item) -> i64 = it.nope";
  chk "ctor unknown field" [ "CA0004" ] "fn f() -> card = card { nope: \"x\" }";
  chk "ctor non-struct" [ "CA0003" ] "fn f() -> pm = pm { x: 1 }";
  chk "unguarded div" [ "CA0005" ] "fn f(a: i64, b: i64) -> i64 = a / b";
  chk "unguarded mod" [ "CA0005" ] "fn f(a: i64, b: i64) -> i64 = a % b";
  chk "match non-sum" [ "CA0006" ]
    "fn f(a: i64) -> i64 = match a { x => 1 Unknown(u) => 0 }";
  chk "unknown variant" [ "CA0007"; "CA0008" ]
    "fn f(p: pm) -> string = match p { nope(c) => \"x\" Unknown(u) => \"?\" }";
  chk "missing variant" [ "CA0008" ]
    "fn f(p: pm) -> string = match p { card(c) => c.last4 Unknown(u) => \"?\" }";
  chk "missing unknown" [ "CA0008" ]
    "fn f(p: pm) -> string = match p { card(c) => c.last4 pix(x) => x.key }";
  chk "divergent arms" [ "CA0009" ]
    "fn f(c: colour) -> i64 = match c { red => 1 green => \"x\" Unknown(u) => \
     0 }";
  chk "recursion" [ "CA0010" ]
    "fn a(x: i64) -> i64 = b(x)\nfn b(x: i64) -> i64 = a(x)";
  chk "self recursion" [ "CA0010" ] "fn a(x: i64) -> i64 = a(x)";
  (* An already-Err operand does not cascade into a second diagnostic. *)
  chk "no cascade" [ "CA0001" ] "fn f() -> i64 = x + 1";
  chk "match err scrutinee" [ "CA0001" ]
    "fn f() -> i64 = match x { a => 1 Unknown(u) => 0 }";
  chk "head wrong arity" [ "CA0002" ]
    "fn f(c: cart) -> item? = head(c.items, 0)";
  chk "find non-function arg" [ "CA0003" ]
    "fn f(xs: []i64) -> i64 = find(xs, 1) ?? 0";
  chk "find wrong predicate" [ "CA0003" ]
    "fn p(a: i64, b: i64) -> bool = true\n\
     fn f(xs: []i64) -> i64 = find(xs, p) ?? 0";
  chk "fold wrong function" [ "CA0003" ]
    "fn g(x: i64) -> i64 = x\nfn f(xs: []i64) -> i64 = fold(xs, 0, g)";
  chk "find predicate not bool" [ "CA0003" ]
    "fn p(x: i64) -> i64 = x\nfn f(xs: []i64) -> i64 = find(xs, p) ?? 0";
  chk "lookup wrong key" [ "CA0003" ]
    "fn f(c: cart) -> i64 = get_or(c.tags, 1, 0)"

(* ── Reference numeric semantics (golden vectors) ──────────────────────── *)

let i64 n = Int64.of_int n
let eqi label exp got = Alcotest.(check int64) label (i64 exp) got

let num_wrap () =
  let open Calc_eval.Num in
  eqi "i8 127+1" (-128) (add ~bits:8 ~signed:true (i64 127) 1L);
  eqi "u8 255+1" 0 (add ~bits:8 ~signed:false (i64 255) 1L);
  eqi "i8 -128-1" 127 (sub ~bits:8 ~signed:true (i64 (-128)) 1L);
  eqi "u8 0-1" 255 (sub ~bits:8 ~signed:false 0L 1L);
  eqi "i16 mul wraps" (-2) (mul ~bits:16 ~signed:true (i64 32767) 2L);
  eqi "u16 wrap" 0 (mul ~bits:16 ~signed:false (i64 65536) 1L);
  eqi "i64 add" 3 (add ~bits:64 ~signed:true 1L 2L)

let num_div () =
  let open Calc_eval.Num in
  let d a b = div ~bits:32 ~signed:true (i64 a) (i64 b) in
  let m a b = rem ~bits:32 ~signed:true (i64 a) (i64 b) in
  eqi "7/3" 2 (d 7 3);
  eqi "7%3" 1 (m 7 3);
  eqi "-7/3" (-2) (d (-7) 3);
  eqi "-7%3" (-1) (m (-7) 3);
  eqi "7/-3" (-2) (d 7 (-3));
  eqi "7%-3" 1 (m 7 (-3));
  eqi "-7/-3" 2 (d (-7) (-3));
  eqi "-7%-3" (-1) (m (-7) (-3));
  Alcotest.(check int64)
    "identity (a/b)*b+a%b" (i64 (-7))
    (Int64.add (Int64.mul (d (-7) 3) 3L) (m (-7) 3));
  eqi "INT_MIN / -1 wraps" (-2147483648)
    (div ~bits:32 ~signed:true (i64 (-2147483648)) (-1L));
  eqi "div by zero is defensive 0" 0 (d 5 0);
  eqi "rem by zero is defensive 0" 0 (m 5 0);
  (* unsigned division differs from signed on the high bit *)
  eqi "u8 200/3" 66 (div ~bits:8 ~signed:false (i64 200) 3L)

let num_coerce () =
  let open Calc_eval.Num in
  let oi = Alcotest.(check (option int64)) in
  oi "to_int 3.9" (Some 3L) (to_int 3.9);
  oi "to_int -3.9" (Some (-3L)) (to_int (-3.9));
  oi "to_int nan" None (to_int nan);
  oi "to_int +inf" None (to_int infinity);
  oi "to_int -inf" None (to_int neg_infinity);
  Alcotest.(check (float 0.0)) "to_float 5" 5.0 (to_float ~signed:true 5L);
  let big = Int64.add (Int64.shift_left 1L 53) 1L in
  Alcotest.(check bool)
    "lossy above 2^53" true
    (to_float ~signed:true big = 9007199254740992.0);
  Alcotest.(check bool)
    "u64 to_float treats bits as unsigned" true
    (to_float ~signed:false (-1L) > 9.0e18)

(* ── Program evaluation ────────────────────────────────────────────────── *)

let prog src = fst (Calc_parser.parse src)
let asint = function Calc_eval.VInt (n, _, _) -> Int64.to_int n | _ -> -999
let asstr = function Calc_eval.VStr s -> s | _ -> "?"

let eval_prog () =
  let open Calc_eval in
  let p = prog "fn f(a: i64, b: i64) -> i64 = a + b * 2" in
  Alcotest.(check int) "arith" 11 (asint (eval_fn p "f" [ vint 3; vint 4 ]));
  (* width-aware wrapping flows through evaluation *)
  let p2 = prog "fn f(a: i8) -> i8 = a + 1" in
  Alcotest.(check int)
    "i8 wraps" (-128)
    (asint (eval_fn p2 "f" [ VInt (127L, 8, true) ]));
  let p3 = prog "fn f(a: i64, b: i64) -> i64 = if b != 0 then a / b else -1" in
  Alcotest.(check int)
    "guarded div" 3
    (asint (eval_fn p3 "f" [ vint 7; vint 2 ]));
  Alcotest.(check int)
    "div guard taken" (-1)
    (asint (eval_fn p3 "f" [ vint 7; vint 0 ]));
  let p4 = prog "fn f(a: i64) -> bool = let y = a in y > 0 && y < 10" in
  Alcotest.(check bool)
    "let, compare, logical" true
    (match eval_fn p4 "f" [ vint 5 ] with VBool b -> b | _ -> false);
  Alcotest.(check bool)
    "logical short-circuit" false
    (match eval_fn p4 "f" [ vint 50 ] with VBool b -> b | _ -> false);
  let p5 =
    prog
      "fn dbl(x: i64) -> i64 = x * 2\n\
       fn pos(x: i64) -> bool = x > 0\n\
       fn add(a: i64, x: i64) -> i64 = a + x\n\
       fn f(xs: []i64) -> i64 = fold(filter(map(xs, dbl), pos), 0, add)"
  in
  Alcotest.(check int)
    "map/filter/fold" 12
    (asint (eval_fn p5 "f" [ VList [ vint 1; vint 2; vint 3 ] ]));
  let p6 =
    prog
      "fn lt2(x: i64) -> bool = x < 2\n\
       fn f(xs: []i64) -> i64 = length(xs) + (head(xs) ?? 0) + (get(xs, 1) ?? \
       0) + (find(xs, lt2) ?? -1)"
  in
  Alcotest.(check int)
    "length/head/get/find" 14
    (asint (eval_fn p6 "f" [ VList [ vint 10; vint 3 ] ]));
  let p7 =
    prog
      "fn f(m: map[string]i64, k: string) -> i64 = get_or(m, k, 0) + \
       (lookup(m, k) ?? 0)"
  in
  Alcotest.(check int)
    "map builtins" 84
    (asint (eval_fn p7 "f" [ VMap [ (VStr "a", vint 42) ]; VStr "a" ]));
  let p8 =
    prog
      "fn f(p: pm) -> string = match p { card(c) => \"C\" pix(x) => \"P\" \
       Unknown(u) => u }"
  in
  Alcotest.(check string)
    "match variant" "C"
    (asstr (eval_fn p8 "f" [ VVariant ("card", VStruct []) ]));
  Alcotest.(check string)
    "match unknown arm" "wire"
    (asstr (eval_fn p8 "f" [ VVariant ("wire", VStruct []) ]));
  let p9 = prog "fn f(x: float) -> float = to_float(to_int(x) ?? 0)" in
  Alcotest.(check bool)
    "coercions and coalesce" true
    (match eval_fn p9 "f" [ VFloat 3.9 ] with
    | VFloat f -> f = 3.0
    | _ -> false);
  let p10 = prog "fn f(a: i64) -> i64? = if a > 0 then Some(a) else None" in
  Alcotest.(check bool)
    "some/none" true
    (match eval_fn p10 "f" [ vint 5 ] with VOpt (Some _) -> true | _ -> false);
  let p11 = prog "fn f() -> card = card { last4: \"1234\" }" in
  Alcotest.(check bool)
    "struct ctor" true
    (match eval_fn p11 "f" [] with
    | VStruct [ ("last4", VStr "1234") ] -> true
    | _ -> false)

let eval_ops () =
  let open Calc_eval in
  let p =
    prog
      "fn sub(a: i64, b: i64) -> i64 = a - b\n\
       fn md(a: i64, b: i64) -> i64 = if b != 0 then a % b else 0\n\
       fn cmp(a: i64, b: i64) -> bool = a <= b && a >= b\n\
       fn ne(a: i64, b: i64) -> bool = a != b\n\
       fn cat(a: string, b: string) -> string = a ++ b\n\
       fn fl(a: float, b: float) -> float = a - b * a\n\
       fn miss(m: map[string]i64) -> i64 = get_or(m, \"z\", -1) + (lookup(m, \
       \"z\") ?? -2)\n\
       fn ti(x: float) -> i64 = to_int(x) ?? -1"
  in
  Alcotest.(check int) "sub" 3 (asint (eval_fn p "sub" [ vint 10; vint 7 ]));
  Alcotest.(check int) "mod" 1 (asint (eval_fn p "md" [ vint 7; vint 3 ]));
  Alcotest.(check bool)
    "cmp <= and >=" true
    (match eval_fn p "cmp" [ vint 5; vint 5 ] with VBool b -> b | _ -> false);
  Alcotest.(check bool)
    "ne" true
    (match eval_fn p "ne" [ vint 1; vint 2 ] with VBool b -> b | _ -> false);
  Alcotest.(check string)
    "concat" "ab"
    (asstr (eval_fn p "cat" [ VStr "a"; VStr "b" ]));
  Alcotest.(check bool)
    "float arith" true
    (match eval_fn p "fl" [ VFloat 2.0; VFloat 3.0 ] with
    | VFloat f -> f = -4.0
    | _ -> false);
  Alcotest.(check int) "map misses" (-3) (asint (eval_fn p "miss" [ VMap [] ]));
  Alcotest.(check int) "to_int nan" (-1) (asint (eval_fn p "ti" [ VFloat nan ]))

(* A well-typed program always terminates and yields a value (AC-15). *)
let totality_test =
  QCheck.Test.make ~count:300 ~name:"totality"
    QCheck.(pair (list (int_range (-1000) 1000)) (int_range (-1000) 1000))
    (fun (xs, n) ->
      let p =
        prog
          "fn dbl(x: i64) -> i64 = x * 2\n\
           fn add(a: i64, x: i64) -> i64 = a + x\n\
           fn f(xs: []i64, n: i64) -> i64 = if n != 0 then fold(map(xs, dbl), \
           0, add) / n else 0"
      in
      match
        Calc_eval.eval_fn p "f"
          [ Calc_eval.VList (List.map Calc_eval.vint xs); Calc_eval.vint n ]
      with
      | Calc_eval.VInt _ -> true
      | _ -> false)

let totality () = QCheck.Test.check_exn totality_test

(* Exercise the type system's rendering and compatibility directly. *)
let types_unit () =
  let open Calc_types in
  let i8 = Prim (Ir.Int { bits = 8; signed = true }) in
  let u32 = Prim (Ir.Int { bits = 32; signed = false }) in
  let all =
    [
      Prim Ir.Bool;
      Prim Ir.String;
      Prim Ir.Bytes;
      i8;
      u32;
      Prim Ir.Float;
      Prim Ir.Timestamp;
      Prim Ir.Date;
      Prim Ir.Duration;
      Prim Ir.Uuid;
      Ref ("x", []);
      Ref ("p", [ Prim Ir.Bool ]);
      List i8;
      Map (Prim Ir.String, i8);
      Opt i8;
      Fn ([ i8 ], Prim Ir.Bool);
      Int_lit;
      Err;
    ]
  in
  List.iter
    (fun t ->
      Alcotest.(check bool) "rendered" true (String.length (to_string t) > 0))
    all;
  let yes l a b = Alcotest.(check bool) l true (compat a b) in
  let no l a b = Alcotest.(check bool) l false (compat a b) in
  yes "err left" Err i8;
  yes "err right" i8 Err;
  yes "intlit~u32" Int_lit u32;
  yes "u32~intlit" u32 Int_lit;
  yes "intlit~intlit" Int_lit Int_lit;
  yes "ref applied" (Ref ("p", [ i8 ])) (Ref ("p", [ i8 ]));
  yes "list" (List i8) (List i8);
  yes "map" (Map (i8, i8)) (Map (i8, i8));
  yes "opt" (Opt i8) (Opt i8);
  yes "fn" (Fn ([ i8 ], i8)) (Fn ([ i8 ], i8));
  no "prim diff" i8 (Prim Ir.Bool);
  no "ref name diff" (Ref ("a", [])) (Ref ("b", []));
  no "ref arity diff" (Ref ("a", [ i8 ])) (Ref ("a", []));
  no "fn arity diff" (Fn ([ i8 ], i8)) (Fn ([], i8));
  no "list vs map" (List i8) (Map (i8, i8));
  Alcotest.(check bool) "numeric float" true (is_numeric (Prim Ir.Float));
  Alcotest.(check bool) "numeric bool" false (is_numeric (Prim Ir.Bool))

let () =
  Alcotest.run "calculus"
    [
      ("types", [ Alcotest.test_case "rendering and compat" `Quick types_unit ]);
      ( "eval",
        [
          Alcotest.test_case "wrapping" `Quick num_wrap;
          Alcotest.test_case "truncated div/mod" `Quick num_div;
          Alcotest.test_case "coercions" `Quick num_coerce;
          Alcotest.test_case "program evaluation" `Quick eval_prog;
          Alcotest.test_case "more operators" `Quick eval_ops;
          Alcotest.test_case "totality" `Quick totality;
        ] );
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
      ( "check",
        [
          Alcotest.test_case "ok" `Quick check_ok;
          Alcotest.test_case "errors" `Quick check_errors;
        ] );
    ]
