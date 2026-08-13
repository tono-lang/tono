open Tono_frontend

(* Malformed-input regression tests for the "ext <name> { ... }" FFI library
   block (Parser_extern), following the recovery-behaviour convention set by
   parse_error_test.ml. The parser never raises; every case below is expected
   to record at least one diagnostic and still return a best-effort AST. *)

let file_diags src = snd (Parser.parse src)
let nonempty name ds = Alcotest.(check bool) name true (List.length ds >= 1)

let contains ~sub s =
  let n = String.length sub and m = String.length s in
  let rec go i = i + n <= m && (String.sub s i n = sub || go (i + 1)) in
  n = 0 || go 0

let has_message ~sub ds =
  List.exists (fun (d : Diagnostic.t) -> contains ~sub d.message) ds

(* Representative cases assert the actual diagnostic message, not just that
   one exists: a regression that fires the wrong diagnostic at the right
   spot would otherwise still pass every other case in this file. *)
let missing_call_line () =
  let ds =
    file_diags
      {|ext mylib {
         go: "example.com/mylib"
         extern load(): string {
           go { yields: (x: string) }
         }
       }|}
  in
  Alcotest.(check bool)
    "names the missing 'call:' line" true
    (has_message ~sub:"requires a 'call:' line" ds)

let empty_yields () =
  let ds =
    file_diags
      {|ext mylib {
         go: "example.com/mylib"
         extern load(): string {
           go { call: "Load"() yields: () }
         }
       }|}
  in
  Alcotest.(check bool)
    "names the empty yields list" true
    (has_message ~sub:"must name at least one binding" ds)

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

(* A foreign struct or opaque type literally named 'error' would collide
   with the yields-position sentinel (it could be declared but never
   referenced); rejected at the declaration site itself. *)
let struct_named_error () =
  let ds =
    file_diags
      {|ext mylib {
        go: "example.com/mylib"
        struct error { message: string }
      }|}
  in
  Alcotest.(check bool)
    "names the collision" true
    (has_message ~sub:"cannot be named 'error'" ds)

let opaque_type_named_error () =
  let ds =
    file_diags
      {|ext mylib {
        go: "example.com/mylib"
        type error { extern send(): string { go { call: "Send"() } } }
      }|}
  in
  Alcotest.(check bool)
    "names the collision" true
    (has_message ~sub:"cannot be named 'error'" ds)

(* A library named after one of the legacy ext-kind words (hook/contract/
   constraint/impl) still dispatches to the legacy grammar (a documented
   limitation: the legacy form always requires a name after the kind word,
   so this can never be a valid legacy declaration either way), but now
   names the collision once up front instead of only surfacing whatever
   cascading errors legacy parsing produces while failing to match a body
   it was never going to recognize. *)
let library_named_like_legacy_kind () =
  let ds = file_diags {|ext hook { ts: "example.com/mylib" }|} in
  Alcotest.(check bool)
    "names the reserved-word collision" true
    (has_message ~sub:"is a reserved ext-kind word here" ds)

let bad_lang_identifier () =
  nonempty "non-identifier language token"
    (file_diags {|ext mylib { 5: "example.com/mylib" }|})

let bad_lang_path_string () =
  nonempty "non-string module path" (file_diags {|ext mylib { go: 5 }|})

let bad_foreign_field_name () =
  nonempty "non-identifier foreign field name"
    (file_diags
       {|ext mylib {
         go: "example.com/mylib"
         struct s { 5: string }
       }|})

let missing_struct_name () =
  nonempty "missing foreign struct name"
    (file_diags
       {|ext mylib {
         go: "example.com/mylib"
         struct { a: string }
       }|})

let struct_body_junk () =
  nonempty "junk in a foreign struct body"
    (file_diags
       {|ext mylib {
         go: "example.com/mylib"
         struct s { @ }
       }|})

let struct_missing_brace () =
  nonempty "foreign struct never closed"
    (file_diags {|ext mylib { go: "example.com/mylib" struct s { a: string |})

let missing_yields_name () =
  nonempty "missing yields binding name"
    (file_diags
       {|ext mylib {
         go: "example.com/mylib"
         extern load(): string {
           go { call: "Load"() yields: (: string) }
         }
       }|})

let yields_trailing_comma () =
  (* No diagnostic expected: a trailing comma before ')' is accepted. *)
  let ds =
    file_diags
      {|ext mylib {
        go: "example.com/mylib"
        extern load(): string {
          go { call: "Load"() yields: (cfg: string,) }
        }
      }|}
  in
  Alcotest.(check int) "no diagnostics" 0 (List.length ds)

let returns_value_bad_shape () =
  nonempty "returns field value neither '.path' nor 'match'"
    (file_diags
       {|ext mylib {
         go: "example.com/mylib"
         extern load(): app_config {
           go { call: "Load"() returns: app_config { endpoint: 5 } }
         }
       }|})

let missing_returns_field_name () =
  nonempty "missing returns field name"
    (file_diags
       {|ext mylib {
         go: "example.com/mylib"
         extern load(): app_config {
           go { call: "Load"() returns: app_config { : .x } }
         }
       }|})

let returns_body_junk () =
  nonempty "junk in a returns body"
    (file_diags
       {|ext mylib {
         go: "example.com/mylib"
         extern load(): app_config {
           go { call: "Load"() returns: app_config { @ } }
         }
       }|})

let returns_missing_brace () =
  nonempty "returns body never closed"
    (file_diags
       {|ext mylib {
         go: "example.com/mylib"
         extern load(): app_config {
           go { call: "Load"() returns: app_config { |})

let errors_sentinel_not_string () =
  nonempty "errors sentinel is not a string"
    (file_diags
       {|ext mylib {
         go: "example.com/mylib"
         extern load(): string {
           go { call: "Load"() errors: { x => overloaded } }
         }
       }|})

let errors_missing_type () =
  nonempty "errors entry missing a type name"
    (file_diags
       {|ext mylib {
         go: "example.com/mylib"
         extern load(): string {
           go { call: "Load"() errors: { "S" => } }
         }
       }|})

let errors_body_junk () =
  nonempty "junk in an errors body"
    (file_diags
       {|ext mylib {
         go: "example.com/mylib"
         extern load(): string {
           go { call: "Load"() errors: { 5 } }
         }
       }|})

let call_symbol_missing () =
  nonempty "call: without a foreign symbol string"
    (file_diags
       {|ext mylib {
         go: "example.com/mylib"
         extern load(): string {
           go { call: 5() }
         }
       }|})

let lang_block_junk () =
  nonempty "junk in a language block"
    (file_diags
       {|ext mylib {
         go: "example.com/mylib"
         extern load(): string {
           go { @ call: "Load"() }
         }
       }|})

let lang_block_missing_brace () =
  nonempty "language block never closed"
    (file_diags
       {|ext mylib {
         go: "example.com/mylib"
         extern load(): string {
           go { call: "Load"()
       }|})

let extern_param_bad_name () =
  nonempty "non-identifier extern parameter name"
    (file_diags
       {|ext mylib {
         go: "example.com/mylib"
         extern load(5: string): string {
           go { call: "Load"() }
         }
       }|})

let extern_missing_name () =
  nonempty "missing extern name"
    (file_diags
       {|ext mylib {
         go: "example.com/mylib"
         extern (): string {
           go { call: "Load"() }
         }
       }|})

let extern_body_junk () =
  nonempty "junk in an extern body"
    (file_diags
       {|ext mylib {
         go: "example.com/mylib"
         extern load(): string { @ }
       }|})

let extern_missing_brace () =
  nonempty "extern body never closed"
    (file_diags {|ext mylib { go: "example.com/mylib" extern load(): string |})

let opaque_type_missing_name () =
  nonempty "missing opaque type name"
    (file_diags
       {|ext mylib {
         go: "example.com/mylib"
         type {
           extern send(): string { go { call: "Send"() } }
         }
       }|})

let opaque_type_body_junk () =
  nonempty "non-extern member in an opaque type body"
    (file_diags
       {|ext mylib {
         go: "example.com/mylib"
         type publisher { @ }
       }|})

let opaque_type_missing_brace () =
  nonempty "opaque type body never closed"
    (file_diags {|ext mylib { go: "example.com/mylib" type publisher |})

let ext_lib_body_junk () =
  nonempty "junk directly in an ext lib body" (file_diags {|ext mylib { @ }|})

let ext_lib_missing_brace () =
  nonempty "ext lib body never closed"
    (file_diags {|ext mylib { go: "example.com/mylib"|})

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
          Alcotest.test_case "body junk" `Quick lang_block_junk;
          Alcotest.test_case "missing brace" `Quick lang_block_missing_brace;
        ] );
      ( "'error' reserved word",
        [
          Alcotest.test_case "as extern return type" `Quick
            error_as_extern_return_type;
          Alcotest.test_case "as returns type" `Quick error_as_returns_type;
          Alcotest.test_case "as ordinary field name" `Quick
            error_as_ordinary_field_name;
          Alcotest.test_case "as a foreign struct name" `Quick
            struct_named_error;
          Alcotest.test_case "as an opaque type name" `Quick
            opaque_type_named_error;
        ] );
      ( "lang path",
        [
          Alcotest.test_case "bad identifier" `Quick bad_lang_identifier;
          Alcotest.test_case "bad path string" `Quick bad_lang_path_string;
        ] );
      ( "foreign struct",
        [
          Alcotest.test_case "bad field name" `Quick bad_foreign_field_name;
          Alcotest.test_case "missing name" `Quick missing_struct_name;
          Alcotest.test_case "body junk" `Quick struct_body_junk;
          Alcotest.test_case "missing brace" `Quick struct_missing_brace;
        ] );
      ( "yields",
        [
          Alcotest.test_case "missing name" `Quick missing_yields_name;
          Alcotest.test_case "trailing comma accepted" `Quick
            yields_trailing_comma;
        ] );
      ( "returns",
        [
          Alcotest.test_case "value bad shape" `Quick returns_value_bad_shape;
          Alcotest.test_case "missing field name" `Quick
            missing_returns_field_name;
          Alcotest.test_case "body junk" `Quick returns_body_junk;
          Alcotest.test_case "missing brace" `Quick returns_missing_brace;
        ] );
      ( "errors",
        [
          Alcotest.test_case "sentinel not a string" `Quick
            errors_sentinel_not_string;
          Alcotest.test_case "missing type" `Quick errors_missing_type;
          Alcotest.test_case "body junk" `Quick errors_body_junk;
        ] );
      ( "call",
        [ Alcotest.test_case "symbol missing" `Quick call_symbol_missing ] );
      ( "extern",
        [
          Alcotest.test_case "bad param name" `Quick extern_param_bad_name;
          Alcotest.test_case "missing name" `Quick extern_missing_name;
          Alcotest.test_case "body junk" `Quick extern_body_junk;
          Alcotest.test_case "missing brace" `Quick extern_missing_brace;
        ] );
      ( "opaque type",
        [
          Alcotest.test_case "missing name" `Quick opaque_type_missing_name;
          Alcotest.test_case "body junk" `Quick opaque_type_body_junk;
          Alcotest.test_case "missing brace" `Quick opaque_type_missing_brace;
        ] );
      ( "ext lib",
        [
          Alcotest.test_case "body junk" `Quick ext_lib_body_junk;
          Alcotest.test_case "missing brace" `Quick ext_lib_missing_brace;
        ] );
      ( "kind dispatch",
        [
          Alcotest.test_case "hook vs library name" `Quick
            kind_dispatch_disambiguation;
          Alcotest.test_case "library named like a legacy kind word" `Quick
            library_named_like_legacy_kind;
        ] );
    ]
