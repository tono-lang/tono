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
         go { #(example.com/mylib) }
         op load(): string {
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
         go { #(example.com/mylib) }
         op load(): string {
           go { call: #(Load)() yields: () }
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
         go { #(example.com/mylib) }
         op load(): string {
           go { call: #(Load)() returns: { } }
         }
       }|})

let error_as_extern_return_type () =
  nonempty "'error' used as an extern return type"
    (file_diags
       {|ext mylib {
         go { #(example.com/mylib) }
         op load(): error {
           go { call: #(Load)() }
         }
       }|})

let error_as_returns_type () =
  nonempty "'error' used as a 'returns:' type"
    (file_diags
       {|ext mylib {
         go { #(example.com/mylib) }
         op load(): string {
           go { call: #(Load)() returns: error { } }
         }
       }|})

(* Outside 'yields:'/'returns:'/an extern's own return type, 'error' is an
   ordinary identifier: a foreign struct may still name a field 'error'. *)
let error_as_ordinary_field_name () =
  let ds =
    file_diags
      {|ext mylib {
        go { #(example.com/mylib) }
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
        go { #(example.com/mylib) }
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
        go { #(example.com/mylib) }
        struct error { op send(): string { go { call: #(Send)() } } }
      }|}
  in
  Alcotest.(check bool)
    "names the collision" true
    (has_message ~sub:"cannot be named 'error'" ds)

(* A library named after one of the legacy ext-kind words (hook/contract/
   constraint/impl) immediately followed by '{' can never be a valid legacy
   declaration either way (that grammar always requires a name after the
   kind word), so this names the collision once up front and skips the
   whole malformed body rather than re-entering the legacy parser and
   cascading into a diagnostic per token while it fails to recover. *)
let library_named_like_legacy_kind () =
  let ds = file_diags {|ext hook { ts: "example.com/mylib" }|} in
  Alcotest.(check bool)
    "names the reserved-word collision" true
    (has_message ~sub:"is a reserved ext-kind word here" ds)

(* The fix for the collision above is a single diagnostic, not a cascade:
   asserting the exact count guards against the legacy parser being
   re-entered on the malformed body (which previously produced dozens of
   "expected an extension name"-shaped errors for one mistake). *)
let library_named_like_legacy_kind_is_a_single_diagnostic () =
  let ds =
    file_diags
      {|ext impl {
         call: "Load"()
         yields: (x: string)
         returns: { }
       }|}
  in
  Alcotest.(check int) "exactly one diagnostic" 1 (List.length ds)

(* A primitive type name (the lexer's own reserved keyword, not an ordinary
   identifier) can never be a legal library name either, and the collision
   is unconditional (unlike the kind-word case above, which is only
   ambiguous with the legacy grammar): "ext uuid { ... }" is the exact case
   a reviewer hit while binding github.com/google/uuid. *)
let library_named_like_a_primitive () =
  let ds = file_diags {|ext uuid { go: "github.com/google/uuid" }|} in
  Alcotest.(check bool)
    "names the primitive collision" true
    (has_message ~sub:"is a reserved primitive type name" ds)

let library_named_like_a_primitive_is_a_single_diagnostic () =
  let ds =
    file_diags
      {|ext string {
         go { #(example.com/mylib) }
         op load(): string {
           go { call: #(Load)() }
         }
       }|}
  in
  Alcotest.(check int) "exactly one diagnostic" 1 (List.length ds)

(* A regular top-level shape (not a foreign struct/opaque type inside an
   'ext' library) named 'error' would collide the same way: a 'yields:'
   position naming it can only ever read the reserved sentinel, never a
   reference to the declared shape. *)
let top_level_struct_named_error () =
  let ds = file_diags {|struct error { message: string }|} in
  Alcotest.(check bool)
    "names the collision" true
    (has_message ~sub:"cannot be named 'error'" ds)

let top_level_union_named_error () =
  let ds = file_diags {|union error { variant(string) }|} in
  Alcotest.(check bool)
    "names the collision" true
    (has_message ~sub:"cannot be named 'error'" ds)

let top_level_enum_named_error () =
  let ds = file_diags {|enum error { a, b }|} in
  Alcotest.(check bool)
    "names the collision" true
    (has_message ~sub:"cannot be named 'error'" ds)

let bad_lang_identifier () =
  nonempty "non-identifier language token"
    (file_diags {|ext mylib { 5: "example.com/mylib" }|})

let bad_lang_path_string () =
  nonempty "non-string module path" (file_diags {|ext mylib { go: 5 }|})

let bad_foreign_field_name () =
  nonempty "non-identifier foreign field name"
    (file_diags
       {|ext mylib {
         go { #(example.com/mylib) }
         struct s { 5: string }
       }|})

let missing_struct_name () =
  nonempty "missing foreign struct name"
    (file_diags
       {|ext mylib {
         go { #(example.com/mylib) }
         struct { a: string }
       }|})

let struct_body_junk () =
  nonempty "junk in a foreign struct body"
    (file_diags
       {|ext mylib {
         go { #(example.com/mylib) }
         struct s { @ }
       }|})

let struct_missing_brace () =
  nonempty "foreign struct never closed"
    (file_diags {|ext mylib { go: "example.com/mylib" struct s { a: string |})

let missing_yields_name () =
  nonempty "missing yields binding name"
    (file_diags
       {|ext mylib {
         go { #(example.com/mylib) }
         op load(): string {
           go { call: #(Load)() yields: (: string) }
         }
       }|})

let yields_trailing_comma () =
  (* No diagnostic expected: a trailing comma before ')' is accepted. *)
  let ds =
    file_diags
      {|ext mylib {
        go { #(example.com/mylib) }
        op load(): string {
          go { call: #(Load)() yields: (cfg: string,) }
        }
      }|}
  in
  Alcotest.(check int) "no diagnostics" 0 (List.length ds)

let returns_value_bad_shape () =
  nonempty "returns field value neither '.path' nor 'match'"
    (file_diags
       {|ext mylib {
         go { #(example.com/mylib) }
         op load(): app_config {
           go { call: #(Load)() returns: app_config { endpoint: 5 } }
         }
       }|})

let missing_returns_field_name () =
  nonempty "missing returns field name"
    (file_diags
       {|ext mylib {
         go { #(example.com/mylib) }
         op load(): app_config {
           go { call: #(Load)() returns: app_config { : .x } }
         }
       }|})

let returns_body_junk () =
  nonempty "junk in a returns body"
    (file_diags
       {|ext mylib {
         go { #(example.com/mylib) }
         op load(): app_config {
           go { call: #(Load)() returns: app_config { @ } }
         }
       }|})

let returns_missing_brace () =
  nonempty "returns body never closed"
    (file_diags
       {|ext mylib {
         go { #(example.com/mylib) }
         op load(): app_config {
           go { call: #(Load)() returns: app_config { |})

let call_symbol_missing () =
  nonempty "call: without a foreign symbol string"
    (file_diags
       {|ext mylib {
         go { #(example.com/mylib) }
         op load(): string {
           go { call: 5() }
         }
       }|})

let lang_block_junk () =
  nonempty "junk in a language block"
    (file_diags
       {|ext mylib {
         go { #(example.com/mylib) }
         op load(): string {
           go { @ call: #(Load)() }
         }
       }|})

let lang_block_missing_brace () =
  nonempty "language block never closed"
    (file_diags
       {|ext mylib {
         go { #(example.com/mylib) }
         op load(): string {
           go { call: #(Load)()
       }|})

let extern_param_bad_name () =
  nonempty "non-identifier extern parameter name"
    (file_diags
       {|ext mylib {
         go { #(example.com/mylib) }
         op load(5: string): string {
           go { call: #(Load)() }
         }
       }|})

let extern_missing_name () =
  nonempty "missing extern name"
    (file_diags
       {|ext mylib {
         go { #(example.com/mylib) }
         op (): string {
           go { call: #(Load)() }
         }
       }|})

let extern_body_junk () =
  nonempty "junk in an extern body"
    (file_diags
       {|ext mylib {
         go { #(example.com/mylib) }
         op load(): string { @ }
       }|})

let extern_missing_brace () =
  nonempty "extern body never closed"
    (file_diags {|ext mylib { go: "example.com/mylib" op load(): string |})

let opaque_type_missing_name () =
  nonempty "missing opaque type name"
    (file_diags
       {|ext mylib {
         go { #(example.com/mylib) }
         type {
           op send(): string { go { call: #(Send)() } }
         }
       }|})

let opaque_type_body_junk () =
  nonempty "non-extern member in an opaque type body"
    (file_diags
       {|ext mylib {
         go { #(example.com/mylib) }
         struct publisher { @ }
       }|})

(* ── language blocks ───────────────────────────────────────────────────── *)

let lang_block_missing_head () =
  let ds =
    file_diags
      {|ext mylib {
         go { #(example.com/mylib) }
         struct source { go { } }
       }|}
  in
  Alcotest.(check bool)
    "names the missing spelling" true
    (has_message ~sub:"first element of a language block" ds)

let lang_block_keyed_entry_missing_spelling () =
  let ds =
    file_diags
      {|ext mylib {
         go { #(example.com/mylib) }
         struct opts { a: string  go { #(Opts) a: string } }
       }|}
  in
  Alcotest.(check bool)
    "names the field's foreign form" true
    (has_message ~sub:"field's foreign form" ds)

let header_block_with_a_keyed_entry () =
  let ds = file_diags {|ext mylib { go { #(example.com/mylib) a: #(x) } }|} in
  Alcotest.(check bool)
    "names the module path rule" true
    (has_message ~sub:"names only the module path" ds)

let call_head_not_a_spelling () =
  let ds =
    file_diags
      {|ext mylib {
         go { #(example.com/mylib) }
         op get(): string { go { call: "Get"() } }
       }|}
  in
  Alcotest.(check bool)
    "names the callee rule" true
    (has_message ~sub:"callee after 'call:'" ds)

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

(* The boundary of call:: a "." after the arguments chains exactly one
   method on the returned object, spelled and called. Each case below is
   something that would make the line an expression instead, and each is
   refused with the message that says where the intended thing goes. *)
let chain_without_call () =
  let ds =
    file_diags
      {|ext mylib {
         go { #(example.com/mylib) }
         op load(key: string): string {
           go { call: #(Get)(key).#(Val) }
         }
       }|}
  in
  Alcotest.(check bool)
    "a bare spelling after the dot is a field read, refused" true
    (has_message ~sub:"is a call and takes '(...)'" ds)

let chain_twice () =
  let ds =
    file_diags
      {|ext mylib {
         go { #(example.com/mylib) }
         op load(key: string): string {
           go { call: #(Get)(key).#(Result)().#(String)() }
         }
       }|}
  in
  Alcotest.(check bool)
    "a second link is refused" true
    (has_message ~sub:"at most one method on the returned object" ds)

let chain_tono_path () =
  let ds =
    file_diags
      {|ext mylib {
         go { #(example.com/mylib) }
         op load(key: string): string {
           go { call: #(Get)(key).val }
         }
       }|}
  in
  Alcotest.(check bool)
    "a tono path on the result is refused" true
    (has_message ~sub:"'call:' takes no expression" ds)

let () =
  Alcotest.run "extern-parse-error"
    [
      ( "call chain",
        [
          Alcotest.test_case "field read after the dot" `Quick
            chain_without_call;
          Alcotest.test_case "second link" `Quick chain_twice;
          Alcotest.test_case "tono path on the result" `Quick chain_tono_path;
        ] );
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
          Alcotest.test_case "as a top-level struct name" `Quick
            top_level_struct_named_error;
          Alcotest.test_case "as a top-level union name" `Quick
            top_level_union_named_error;
          Alcotest.test_case "as a top-level enum name" `Quick
            top_level_enum_named_error;
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
      ("errors", []);
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
          Alcotest.test_case "lang block missing head" `Quick
            lang_block_missing_head;
          Alcotest.test_case "keyed entry missing spelling" `Quick
            lang_block_keyed_entry_missing_spelling;
          Alcotest.test_case "header block with a keyed entry" `Quick
            header_block_with_a_keyed_entry;
          Alcotest.test_case "call head not a spelling" `Quick
            call_head_not_a_spelling;
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
          Alcotest.test_case "reserved-word collision is a single diagnostic"
            `Quick library_named_like_legacy_kind_is_a_single_diagnostic;
          Alcotest.test_case "library named like a primitive type" `Quick
            library_named_like_a_primitive;
          Alcotest.test_case "primitive collision is a single diagnostic" `Quick
            library_named_like_a_primitive_is_a_single_diagnostic;
        ] );
    ]
