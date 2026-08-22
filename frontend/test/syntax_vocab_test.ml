open Tono_frontend

(* The construct vocabulary is what the lexer and parser accept, no more and
   no less: every reserved word lexes as a keyword token, every contextual
   word parses in its position, and no other identifier is reserved. The
   editor's docs are gated against this list, so a stale entry here would let
   the two agree while the language disagrees. *)

let errors_of ds =
  List.filter_map
    (fun (d : Diagnostic.t) ->
      if d.severity = Diagnostic.Error then Some d.message else None)
    ds

let parses_clean src =
  Alcotest.(check (list string))
    ("parses: " ^ src) []
    (errors_of (snd (Parser.parse src)))

let keywords_lex_as_keywords () =
  List.iter
    (fun word ->
      let toks, _ = Lexer.tokenize word in
      match toks with
      | { Token.kind = Token.Ident _; _ } :: _ ->
          Alcotest.failf
            "'%s' is listed as a keyword but lexes as an identifier" word
      | { Token.kind; _ } :: _ ->
          Alcotest.(check bool)
            ("the lexer maps " ^ word) true
            (List.assoc_opt word Token.keywords = Some kind)
      | [] -> Alcotest.failf "no token for '%s'" word)
    Syntax_vocab.keywords

let contextual_words_lex_as_identifiers () =
  List.iter
    (fun word ->
      match fst (Lexer.tokenize word) with
      | { Token.kind = Token.Ident w; _ } :: _ when w = word -> ()
      | _ ->
          Alcotest.failf "contextual word '%s' must lex as an identifier" word)
    Syntax_vocab.contextual

(* One snippet per contextual word, at the position the parser reads it. *)
let snippet_for = function
  | "contract" ->
      "ext contract sign(string) -> string { go: \"ext/go/s.go#Sign\" }"
  | "constraint" ->
      "ext constraint positive(i32) -> bool { go: \"ext/go/c.go#Positive\" }"
  | "impl" -> "ext impl save { go: \"ext/go/s.go#Save\" }"
  | "raw" -> "ext impl save raw { go: \"ext/go/s.go#Save\" }"
  | "match" ->
      "pub struct client {\n\
      \  v: string @env(\"V\")\n\
      \  e: string = match .v { \"a\" => \"x\" _ => \"y\" }\n\
       }"
  | "null" ->
      "pub struct client {\n\
      \  by_segment: map[string]string @env(\"BS\")\n\
      \  seg: string @env(\"SEG\")\n\
      \  e: string = match .by_segment[.seg] { null => \"x\" _ => ._ }\n\
       }"
  | "stub" ->
      "test \"t\" {\n\
      \  c: client { }\n\
      \  s: stub c.get.http: [http.response { status: 200 }]\n\
       }"
  | "expect" -> "test \"t\" {\n  c: client { }\n  expect c: user { .. }\n}"
  | word -> Alcotest.failf "no parser snippet for contextual word '%s'" word

let contextual_words_parse_in_position () =
  List.iter
    (fun word -> parses_clean (snippet_for word))
    Syntax_vocab.contextual

let constructs_is_the_union () =
  Alcotest.(check (list string))
    "keywords then contextual"
    (Syntax_vocab.keywords @ Syntax_vocab.contextual)
    Syntax_vocab.constructs;
  Alcotest.(check bool) "is_construct hit" true (Syntax_vocab.is_construct "op");
  Alcotest.(check bool)
    "is_construct miss" false
    (Syntax_vocab.is_construct "hook")

(* The ext block's vocabulary is closed and this test fixes it, not just its
   completeness: a new word (or trait, or target) fails here by construction
   and has to come with an amendment to the RFC that names it. *)
let ext_vocabulary_is_pinned () =
  Alcotest.(check (list string))
    "language block lines"
    [ "call"; "yields"; "returns" ]
    Ext_lib_vocab.lang_fields;
  Alcotest.(check (list string))
    "traits an ext op accepts"
    [ "async"; "errors"; "doc" ]
    Ext_lib_vocab.op_traits;
  Alcotest.(check (list string))
    "targets with an asynchronous call" [ "ts"; "rust" ]
    Ext_lib_vocab.async_targets;
  Alcotest.(check (list string))
    "targets a block can name" [ "go"; "ts"; "rust" ] Ext_lib_vocab.targets;
  Alcotest.(check string)
    "reserved yields position" "error" Ext_lib_vocab.error_sentinel;
  Alcotest.(check string)
    "request reference" "request" Ext_lib_vocab.request_ref;
  (* The declarations of the block are keywords the lexer already owns;
     nothing contextual opens one. *)
  List.iter
    (fun w ->
      Alcotest.(check bool)
        (w ^ " is a keyword") true
        (List.mem_assoc w Token.keywords))
    [ "ext"; "struct"; "op" ];
  (* A stray word inside a language block is refused by the parser, naming
     the three lines it accepts. *)
  let _, diags =
    Parser.parse
      "ext lib {\n\
      \  go { #(x) }\n\
      \  op f(): string {\n\
      \    go { call: #(F)() sync }\n\
      \  }\n\
       }"
  in
  Alcotest.(check bool)
    "a marker word is refused" true
    (List.exists
       (fun (d : Diagnostic.t) ->
         String.equal d.message
           "unexpected token in a language block: expected 'call', 'yields', \
            'returns'")
       diags)

let () =
  Alcotest.run "syntax_vocab"
    [
      ( "vocabulary",
        [
          Alcotest.test_case "keywords lex as keywords" `Quick
            keywords_lex_as_keywords;
          Alcotest.test_case "contextual words lex as identifiers" `Quick
            contextual_words_lex_as_identifiers;
          Alcotest.test_case "contextual words parse in position" `Quick
            contextual_words_parse_in_position;
          Alcotest.test_case "constructs is the union" `Quick
            constructs_is_the_union;
          Alcotest.test_case "ext vocabulary is pinned" `Quick
            ext_vocabulary_is_pinned;
        ] );
    ]
