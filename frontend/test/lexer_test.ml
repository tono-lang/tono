open Tono_frontend

let toks_of src = fst (Lexer.tokenize src)
let diags_of src = snd (Lexer.tokenize src)

let show_kind : Token.kind -> string = function
  | KwStruct -> "struct"
  | KwEnum -> "enum"
  | KwUnion -> "union"
  | KwOp -> "op"
  | KwMap -> "map"
  | KwPub -> "pub"
  | KwThrows -> "throws"
  | Ident s -> "id:" ^ s
  | Prim s -> "prim:" ^ s
  | Str s -> "str:" ^ s
  | Int n -> "int:" ^ string_of_int n
  | At -> "@"
  | LBrace -> "{"
  | RBrace -> "}"
  | LBracket -> "["
  | RBracket -> "]"
  | LParen -> "("
  | RParen -> ")"
  | Colon -> ":"
  | Question -> "?"
  | Comma -> ","
  | Dot -> "."
  | Eq -> "="
  | Arrow -> "->"
  | Eof -> "eof"

let kinds src = List.map (fun (t : Token.t) -> show_kind t.kind) (toks_of src)

let str_contents src =
  List.filter_map
    (fun (t : Token.t) -> match t.kind with Token.Str s -> Some s | _ -> None)
    (toks_of src)

(* ── Tokens and spans ──────────────────────────────────────────────────── *)

let all_kinds () =
  Alcotest.(check (list string))
    "every token kind"
    [
      "struct";
      "enum";
      "union";
      "op";
      "map";
      "pub";
      "throws";
      "id:name";
      "prim:i64";
      "str:s";
      "int:5";
      "@";
      "{";
      "}";
      "[";
      "]";
      "(";
      ")";
      ":";
      "?";
      ",";
      ".";
      "=";
      "->";
      "eof";
    ]
    (kinds
       {|struct enum union op map pub throws name i64 "s" 5 @ { } [ ] ( ) : ? , . = ->|})

let member_tokens () =
  Alcotest.(check (list string))
    "pub struct member"
    [
      "pub";
      "struct";
      "id:charge";
      "{";
      "id:amount";
      ":";
      "prim:i64";
      "}";
      "eof";
    ]
    (kinds {|pub struct charge { amount: i64 }|})

let spans () =
  match toks_of "struct\n  x" with
  | t0 :: t1 :: _ ->
      Alcotest.(check int) "t0 line" 1 t0.span.start.line;
      Alcotest.(check int) "t0 col" 1 t0.span.start.col;
      Alcotest.(check int) "t0 offset" 0 t0.span.start.offset;
      Alcotest.(check int) "t0 finish col" 7 t0.span.finish.col;
      Alcotest.(check int) "t1 line" 2 t1.span.start.line;
      Alcotest.(check int) "t1 col" 3 t1.span.start.col;
      Alcotest.(check int) "t1 offset" 9 t1.span.start.offset
  | _ -> Alcotest.fail "expected at least two tokens"

(* ── Comments ──────────────────────────────────────────────────────────── *)

let comments_discarded () =
  Alcotest.(check (list string))
    "comments gone"
    [ "struct"; "id:x"; "{"; "}"; "eof" ]
    (kinds "// hello\nstruct x {\n  // inner\n}")

(* ── Strings ───────────────────────────────────────────────────────────── *)

let single_string () =
  Alcotest.(check (list string))
    "decoded escapes" [ "a\nb\t\r\"c\\d" ]
    (str_contents {|"a\nb\t\r\"c\\d"|})

let misc_chars () =
  (* Uppercase identifier start, tab/CR whitespace, and a lone quote at EOF. *)
  Alcotest.(check (list string))
    "uppercase ident + tab/cr ws"
    [ "id:Charge"; "id:y"; "eof" ]
    (kinds "Charge\t\r y");
  let toks, ds = Lexer.tokenize "\"" in
  Alcotest.(check bool) "lone quote diagnosed" true (List.length ds >= 1);
  Alcotest.(check bool)
    "lone quote yields a string token" true
    (List.exists
       (fun (t : Token.t) ->
         match t.kind with Token.Str _ -> true | _ -> false)
       toks)

let triple_string () =
  Alcotest.(check (list string))
    "raw multi-line" [ "line1\nli\"ne2" ]
    (str_contents {|"""line1
li"ne2"""|})

(* ── Integers ──────────────────────────────────────────────────────────── *)

let int_literals () =
  Alcotest.(check (list string))
    "status arg"
    [ "@"; "id:status"; "("; "int:402"; ")"; "eof" ]
    (kinds "@status(402)")

let int_overflow () =
  Alcotest.(check bool)
    "overflow diagnosed" true
    (List.length (diags_of "999999999999999999999999999999") >= 1)

(* ── Lexical error recovery ────────────────────────────────────────────── *)

let has_str toks =
  List.exists
    (fun (t : Token.t) -> match t.kind with Token.Str _ -> true | _ -> false)
    toks

let unterminated_single_eof () =
  let toks, ds = Lexer.tokenize {|"abc|} in
  Alcotest.(check bool) "diagnosed" true (List.length ds >= 1);
  Alcotest.(check bool) "still produced a string token" true (has_str toks)

let unterminated_single_newline () =
  let toks, ds = Lexer.tokenize "\"abc\nstruct x {}" in
  Alcotest.(check bool) "diagnosed" true (List.length ds >= 1);
  (* Recovers: scanning continues and finds the struct keyword afterwards. *)
  Alcotest.(check bool)
    "recovered to struct" true
    (List.exists (fun (t : Token.t) -> t.kind = Token.KwStruct) toks)

let unterminated_triple () =
  let _, ds = Lexer.tokenize {|"""abc|} in
  Alcotest.(check bool) "triple diagnosed" true (List.length ds >= 1)

let backslash_at_eof () =
  let _, ds = Lexer.tokenize "\"a\\" in
  Alcotest.(check bool) "diagnosed" true (List.length ds >= 1)

let invalid_escape () =
  let toks, ds = Lexer.tokenize {|"a\qb"|} in
  Alcotest.(check bool) "escape diagnosed" true (List.length ds >= 1);
  Alcotest.(check (list string))
    "kept char literally" [ "aqb" ]
    (List.filter_map
       (fun (t : Token.t) ->
         match t.kind with Token.Str s -> Some s | _ -> None)
       toks)

let unexpected_char () =
  let _, ds = Lexer.tokenize "struct $ x" in
  Alcotest.(check bool) "unexpected char diagnosed" true (List.length ds >= 1)

let empty_source () =
  Alcotest.(check (list string)) "just eof" [ "eof" ] (kinds "");
  Alcotest.(check int) "no diagnostics" 0 (List.length (diags_of "   \n  "))

let arrow_and_dash () =
  Alcotest.(check (list string)) "arrow token" [ "->"; "eof" ] (kinds "->");
  (* A bare '-' (not followed by '>') is an unexpected character. *)
  let _, ds = Lexer.tokenize "a - b" in
  Alcotest.(check bool) "lone dash diagnosed" true (List.length ds >= 1)

let () =
  Alcotest.run "lexer"
    [
      ( "tokens",
        [
          Alcotest.test_case "all kinds" `Quick all_kinds;
          Alcotest.test_case "member tokens" `Quick member_tokens;
          Alcotest.test_case "spans" `Quick spans;
          Alcotest.test_case "comments discarded" `Quick comments_discarded;
          Alcotest.test_case "empty source" `Quick empty_source;
          Alcotest.test_case "arrow and dash" `Quick arrow_and_dash;
        ] );
      ( "strings",
        [
          Alcotest.test_case "single decoded" `Quick single_string;
          Alcotest.test_case "misc chars" `Quick misc_chars;
          Alcotest.test_case "triple raw" `Quick triple_string;
          Alcotest.test_case "invalid escape" `Quick invalid_escape;
        ] );
      ( "integers",
        [
          Alcotest.test_case "int literal" `Quick int_literals;
          Alcotest.test_case "overflow" `Quick int_overflow;
        ] );
      ( "recovery",
        [
          Alcotest.test_case "unterminated single (eof)" `Quick
            unterminated_single_eof;
          Alcotest.test_case "unterminated single (newline)" `Quick
            unterminated_single_newline;
          Alcotest.test_case "unterminated triple" `Quick unterminated_triple;
          Alcotest.test_case "backslash at eof" `Quick backslash_at_eof;
          Alcotest.test_case "unexpected char" `Quick unexpected_char;
        ] );
    ]
