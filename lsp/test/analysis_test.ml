open Lsp.Types
module Analysis = Tono_lsp_lib.Analysis

let contains s sub =
  let ls = String.length s and lsub = String.length sub in
  let rec go i = i + lsub <= ls && (String.sub s i lsub = sub || go (i + 1)) in
  lsub = 0 || go 0

let pos line character = Position.create ~line ~character

(* Two shapes on separate lines; the reference `edge` sits on line 0, its
   declaration on line 1 at column 7 ("struct edge"). *)
let two_shapes = "struct point { at: edge }\nstruct edge { x: i64 }"

let diagnostics_clean () =
  let ds = Analysis.lsp_diagnostics "struct point { x: i64 }" in
  Alcotest.(check int) "no diagnostics for a clean file" 0 (List.length ds)

let diagnostics_error () =
  (* `missing` resolves to nothing: a typecheck error. *)
  let ds = Analysis.lsp_diagnostics "struct box { it: missing }" in
  Alcotest.(check bool) "at least one diagnostic" true (ds <> []);
  let d = List.hd ds in
  Alcotest.(check bool)
    "severity is Error" true
    (d.Diagnostic.severity = Some DiagnosticSeverity.Error);
  Alcotest.(check int) "on the first line" 0 d.Diagnostic.range.start.line

(* An unknown trait reaches the editor: the frontend reports it, so it rides the
   same path every diagnostic does and arrives as a warning (not an error, the
   spec still compiles) carrying its code, which is what a quick fix keys on. *)
let diagnostics_unknown_trait () =
  let ds = Analysis.lsp_diagnostics {|@statusx("x") struct point { x: i64 }|} in
  let d =
    List.find
      (fun (d : Diagnostic.t) -> d.Diagnostic.code = Some (`String "TC0054"))
      ds
  in
  Alcotest.(check bool)
    "severity is Warning" true
    (d.Diagnostic.severity = Some DiagnosticSeverity.Warning);
  let message =
    match d.Diagnostic.message with
    | `String m -> m
    | `MarkupContent m -> m.value
  in
  Alcotest.(check bool)
    "names the trait it was probably meant to be" true
    (contains message "did you mean @status")

let definition_resolves () =
  let uri = Lsp.Uri.of_string "file:///x.tono" in
  let file = Analysis.parse two_shapes in
  (* Cursor inside the `edge` reference on line 0. *)
  match Analysis.definition_at ~uri ~text:two_shapes ~file (pos 0 20) with
  | None -> Alcotest.fail "expected a definition location"
  | Some loc ->
      Alcotest.(check string)
        "same document" (Lsp.Uri.to_string uri)
        (Lsp.Uri.to_string loc.Location.uri);
      Alcotest.(check int)
        "points at the declaration line" 1 loc.Location.range.start.line;
      Alcotest.(check int)
        "at the name column" 7 loc.Location.range.start.character

let definition_miss () =
  let uri = Lsp.Uri.of_string "file:///x.tono" in
  let file = Analysis.parse two_shapes in
  (* Column 0 is the `struct` keyword, not a type reference. *)
  match Analysis.definition_at ~uri ~text:two_shapes ~file (pos 0 0) with
  | None -> ()
  | Some _ -> Alcotest.fail "keyword position is not a reference"

(* The hover markup value at a position, or a test failure. *)
let hover_value ?(markdown = false) (src : string) (p : Position.t) : string =
  let file = Analysis.parse src in
  match Analysis.hover_at ~markdown ~text:src ~file p with
  | None -> Alcotest.fail "expected a hover"
  | Some h -> (
      match h.Hover.contents with
      | `MarkupContent mc -> mc.MarkupContent.value
      | _ -> Alcotest.fail "expected markup content")

let hover_decl () =
  let v = hover_value two_shapes (pos 0 8) in
  Alcotest.(check bool)
    "prints the full declaration" true
    (contains v "struct point {")

let completion_labels (src : string) (p : Position.t) : string list =
  let file = Analysis.parse src in
  List.map
    (fun (c : CompletionItem.t) -> c.label)
    (Analysis.completions ~text:src ~file p)

let completions_list () =
  let labels = completion_labels two_shapes (pos 0 0) in
  Alcotest.(check bool) "declared shape point" true (List.mem "point" labels);
  Alcotest.(check bool) "declared shape edge" true (List.mem "edge" labels);
  Alcotest.(check bool) "primitive i64" true (List.mem "i64" labels)

let completions_offer_declaration_keywords () =
  (* A blank document can only start a declaration, so the starters lead; a
     lone `pub` narrows to what may follow it; past the starter, keywords get
     out of the way of declared names. *)
  let blank = completion_labels "" (pos 0 0) in
  Alcotest.(check bool) "struct at start" true (List.mem "struct" blank);
  Alcotest.(check bool) "pub at start" true (List.mem "pub" blank);
  let after_pub = completion_labels "pub " (pos 0 4) in
  Alcotest.(check bool) "struct after pub" true (List.mem "struct" after_pub);
  Alcotest.(check bool) "no pub after pub" false (List.mem "pub" after_pub);
  let mid = completion_labels "struct point { x: i64 }" (pos 0 10) in
  Alcotest.(check bool)
    "no starter mid-declaration" false (List.mem "struct" mid);
  let ty = completion_labels "struct point { x: " (pos 0 18) in
  Alcotest.(check bool) "map in type position" true (List.mem "map" ty)

let offset_roundtrip () =
  (* Line 1 char 7 in [two_shapes] is the `e` of the second `edge`. *)
  let off = Analysis.offset_of_position two_shapes (pos 1 7) in
  Alcotest.(check char) "lands on the declaration name" 'e' two_shapes.[off]

let document_symbols_outline () =
  let file = Analysis.parse two_shapes in
  let syms = Analysis.document_symbols ~text:two_shapes ~file in
  let names = List.map (fun (s : DocumentSymbol.t) -> s.name) syms in
  Alcotest.(check (list string))
    "one symbol per shape" [ "point"; "edge" ] names;
  let point = List.hd syms in
  Alcotest.(check bool)
    "point is a struct" true
    (point.DocumentSymbol.kind = SymbolKind.Struct);
  let children =
    Option.value ~default:[] point.DocumentSymbol.children
    |> List.map (fun (s : DocumentSymbol.t) -> s.name)
  in
  Alcotest.(check (list string)) "member as child" [ "at" ] children

let references_include_and_exclude_decl () =
  let uri = Lsp.Uri.of_string "file:///x.tono" in
  let file = Analysis.parse two_shapes in
  let at = pos 0 20 in
  let with_decl =
    Analysis.references_at ~uri ~text:two_shapes ~file ~include_decl:true at
  in
  Alcotest.(check int) "declaration plus one use" 2 (List.length with_decl);
  let without =
    Analysis.references_at ~uri ~text:two_shapes ~file ~include_decl:false at
  in
  Alcotest.(check int) "use only" 1 (List.length without)

let rename_edits_every_occurrence () =
  let uri = Lsp.Uri.of_string "file:///x.tono" in
  let file = Analysis.parse two_shapes in
  let we =
    Analysis.rename_at ~uri ~text:two_shapes ~file ~new_name:"border" (pos 0 20)
  in
  match we.WorkspaceEdit.changes with
  | Some [ (_uri, edits) ] ->
      Alcotest.(check int) "declaration and use edited" 2 (List.length edits);
      Alcotest.(check bool)
        "carries the new name" true
        (List.for_all (fun (e : TextEdit.t) -> e.newText = "border") edits)
  | _ -> Alcotest.fail "expected a single-document change set"

let formatting_uses_the_printer () =
  match Analysis.formatting ~text:"struct point { x: i64, y: i64 }" with
  | Some [ edit ] ->
      Alcotest.(check bool)
        "canonical layout" true
        (contains edit.TextEdit.newText "struct point {\n  x: i64\n  y: i64\n}")
  | _ -> Alcotest.fail "expected one formatting edit"

let formatting_declines_on_parse_error () =
  Alcotest.(check bool)
    "no edit for unparsable source" true
    (Analysis.formatting ~text:"struct {" = None)

(* "ação" is 6 bytes but 4 UTF-16 code units, so byte and UTF-16 columns
   diverge after the doc string: `point` sits at byte 22 and UTF-16 column 20.
   Positions must convert through the UTF-16 view in both directions. *)
let accented =
  "@doc(\"ação\") struct point { at: edge }\nstruct edge { x: i64 }"

let utf16_offset_of_position () =
  let off = Analysis.offset_of_position accented (pos 0 20) in
  Alcotest.(check char)
    "utf16 column decodes to the byte of p" 'p' accented.[off]

let utf16_hover_range () =
  let file = Analysis.parse accented in
  match Analysis.hover_at ~markdown:false ~text:accented ~file (pos 0 20) with
  | None -> Alcotest.fail "hover lands on the declaration name"
  | Some h -> (
      match h.Hover.range with
      | None -> Alcotest.fail "hover carries a range"
      | Some r ->
          Alcotest.(check int)
            "range starts at the utf16 column" 20 r.Range.start.character)

let utf16_rename_range () =
  let uri = Lsp.Uri.of_string "file:///x.tono" in
  let file = Analysis.parse accented in
  let we =
    Analysis.rename_at ~uri ~text:accented ~file ~new_name:"node" (pos 0 20)
  in
  match we.WorkspaceEdit.changes with
  | Some [ (_uri, edit :: _) ] ->
      Alcotest.(check int)
        "edit starts at the utf16 column" 20 edit.TextEdit.range.start.character
  | _ -> Alcotest.fail "expected a single-document change set"

(* --- hover content layer --- *)

let documented =
  "@doc(\"The point.\") struct point { at: edge }\nstruct edge { x: i64 }"

let hover_markdown_decl () =
  let v = hover_value ~markdown:true documented (pos 0 27) in
  Alcotest.(check bool) "fenced tono block" true (contains v "```tono");
  Alcotest.(check bool)
    "prints the declaration" true
    (contains v "struct point");
  Alcotest.(check bool) "doc renders as prose" true (contains v "The point.");
  Alcotest.(check bool)
    "the doc trait stays out of the code block" false (contains v "@doc(")

let hover_member () =
  let v = hover_value two_shapes (pos 0 15) in
  Alcotest.(check bool) "name and type" true (contains v "at: edge")

let hover_type_ref_shows_target_decl () =
  let v = hover_value two_shapes (pos 0 20) in
  Alcotest.(check bool)
    "the full target declaration" true
    (contains v "struct edge {")

let hover_keyword () =
  let v = hover_value two_shapes (pos 0 2) in
  Alcotest.(check bool)
    "explains the construct" true
    (contains v "record shape")

(* The 'ext' construct hover no longer documents a lifecycle: hooks are
   removed, so its prose points at the ext/extern FFI model instead. *)
let hover_ext_word_no_longer_lists_a_lifecycle () =
  let src = "ext hook before_request {\n  ts: \"ext/a.ts#f\"\n}" in
  let v = hover_value src (pos 0 1) in
  Alcotest.(check bool)
    "no lifecycle slot in the hover" false (contains v "client_init");
  Alcotest.(check bool) "points at extern" true (contains v "extern")

let hover_primitive () =
  let v = hover_value two_shapes (pos 1 18) in
  Alcotest.(check bool)
    "explains the wire form" true
    (contains v "string on the wire")

let hover_nullable_marker () =
  (* At the exact boundary the left token wins (end-inclusive resolution), so
     the marker is addressed one column in. *)
  let v = hover_value "struct s { n: string? }" (pos 0 21) in
  Alcotest.(check bool)
    "explains two-state nullability" true
    (contains v "present or absent")

let hover_trait_contract () =
  let src =
    "op create(point): point\n\
    \  @http(method: \"POST\", path: \"/points\")\n\
     struct point { x: i64 }"
  in
  let v = hover_value src (pos 1 4) in
  Alcotest.(check bool) "lists the valid keys" true (contains v "method")

(* --- contextual completion --- *)

let completion_after_at () =
  let labels = completion_labels "struct s { x: i64 }\n@" (pos 1 1) in
  Alcotest.(check bool) "offers traits" true (List.mem "http" labels);
  Alcotest.(check bool) "no shapes here" false (List.mem "s" labels)

(* No lifecycle-slot completion is offered anymore: 'hook' does not appear
   among the extension kinds after 'ext', and there is no slot-name position
   to complete after it. *)
let completion_no_hook_slots_offered () =
  let labels = completion_labels "ext " (pos 0 4) in
  Alcotest.(check bool) "no hook kind" false (List.mem "hook" labels);
  Alcotest.(check bool)
    "contract kind offered" true
    (List.mem "contract" labels)

let completion_type_position () =
  let labels = completion_labels "struct s { x: " (pos 0 14) in
  Alcotest.(check bool) "primitives" true (List.mem "i64" labels);
  Alcotest.(check bool) "declared shapes" true (List.mem "s" labels);
  Alcotest.(check bool) "no traits" false (List.mem "http" labels)

let completion_op_input_is_a_type_position () =
  let labels = completion_labels "op f(" (pos 0 5) in
  Alcotest.(check bool) "primitives in op input" true (List.mem "i64" labels)

(* End-inclusive resolution: hover at a reference's end column answers the
   same as one column earlier. *)
let hover_at_token_end () =
  (* `edge` in [two_shapes] spans columns 19-23 on line 0. *)
  Alcotest.(check string)
    "end column resolves the token to its left"
    (hover_value two_shapes (pos 0 22))
    (hover_value two_shapes (pos 0 23))

let formatting_identity_is_no_edit () =
  let canonical = "struct point {\n  x: i64\n}\n" in
  Alcotest.(check bool)
    "already-canonical input formats to no edits" true
    (Analysis.formatting ~text:canonical = Some [])

(* ── The entry model ──────────────────────────────────────────────────── *)

(* One entry exercising every new form the editor has to explain: stacked
   sources, a catalog pipeline, a selection table, a composition binding, and
   an operation with the protocol vocabulary. *)
let entry_src =
  "pub struct client {\n\
  \  api_key: string @arg @str::trim\n\
  \  version: string @env(\"V\") @default(\"v2\")\n\
  \  endpoint: string = match .version {\n\
  \    \"v1\" => .api_key\n\
  \    _ => .api_key\n\
  \  }\n\
  \  conf: settings @bind(api_key, .api_key)\n\n\
  \  op fetch(note): note\n\
  \    @http(method: \"GET\", path: \"/n\", endpoint: .endpoint)\n\
   }\n"

let hover_value_source () =
  (* `@arg` on line 1. *)
  let v = hover_value entry_src (pos 1 20) in
  Alcotest.(check bool)
    "explains the positional argument" true (contains v "positional");
  (* `@env` on line 2. *)
  let v = hover_value entry_src (pos 2 20) in
  Alcotest.(check bool)
    "explains the fallback chain" true (contains v "fallback")

let hover_str_catalog () =
  (* `@str::trim` on line 1. *)
  let v = hover_value entry_src (pos 1 27) in
  Alcotest.(check bool) "names the transform" true (contains v "whitespace");
  Alcotest.(check bool) "names the catalog" true (contains v "@str::")

let hover_match_keyword () =
  (* `match` on line 3. *)
  let v = hover_value entry_src (pos 3 22) in
  Alcotest.(check bool) "explains exhaustiveness" true (contains v "exhaustive");
  Alcotest.(check bool)
    "names the sources an arm can stack" true (contains v "@default")

let hover_field_shows_its_selection_table () =
  (* The `endpoint` field name on line 3. *)
  let v = hover_value entry_src (pos 3 4) in
  Alcotest.(check bool) "renders the match" true (contains v "match .version");
  Alcotest.(check bool) "renders an arm" true (contains v "=>")

let hover_entry_operation () =
  (* The op name on line 9. *)
  let v = hover_value entry_src (pos 9 6) in
  Alcotest.(check bool) "prints the operation" true (contains v "op fetch");
  (* Its @http trait on line 10: nested ops carry traits like any other. *)
  let v = hover_value entry_src (pos 10 6) in
  Alcotest.(check bool) "explains the transport" true (contains v "HTTP")

let hover_raw_word () =
  let src = "ext impl client.save raw {\n  go: \"ext/go/a.go#Save\"\n}\n" in
  let v = hover_value src (pos 0 22) in
  Alcotest.(check bool) "explains the outcome" true (contains v "outcome")

let completion_after_at_offers_value_sources () =
  let labels = completion_labels "struct client { x: string @" (pos 0 27) in
  List.iter
    (fun name ->
      Alcotest.(check bool) ("offers @" ^ name) true (List.mem name labels))
    [ "arg"; "with"; "env"; "default"; "format"; "bind" ]

let completion_after_catalog_separator () =
  let labels =
    completion_labels "struct client { x: string @str::" (pos 0 32)
  in
  Alcotest.(check bool)
    "offers a transform" true
    (List.mem "upper_snake" labels);
  Alcotest.(check bool)
    "only the catalog" false
    (List.mem "http" labels || List.mem "i64" labels)

let completion_after_ext_offers_kinds () =
  let labels = completion_labels "ext " (pos 0 4) in
  Alcotest.(check int) "exactly the kinds" 3 (List.length labels);
  Alcotest.(check bool) "impl among them" true (List.mem "impl" labels);
  Alcotest.(check bool) "hook is not" false (List.mem "hook" labels)

let completion_after_ext_impl_offers_entry_ops () =
  let src = entry_src ^ "\next impl " in
  let line = List.length (String.split_on_char '\n' src) - 1 in
  let labels = completion_labels src (pos line 10) in
  Alcotest.(check bool) "the bare name" true (List.mem "fetch" labels);
  Alcotest.(check bool)
    "the qualified name" true
    (List.mem "client.fetch" labels)

let completion_after_dot_offers_sibling_fields () =
  let src = "pub struct client {\n  api_key: string @arg\n  e: string @env(." in
  let labels = completion_labels src (pos 2 18) in
  Alcotest.(check bool) "a sibling field" true (List.mem "api_key" labels);
  Alcotest.(check bool)
    "not a type position" false
    (List.mem "i64" labels || List.mem "client" labels)

(* The editor's trait vocabulary is the compiler's. The registry carries prose
   the compiler has no use for, so it cannot be derived outright, but it must
   never fall behind: a trait the checkers read with nothing to say about it
   would hover blank and never be offered, and one documented here that the
   compiler does not read would be advertised and then warned about. *)
let trait_docs_cover_the_compiler_vocabulary () =
  let documented = List.map fst Tono_lsp_lib.Hover_docs.trait_registry in
  let missing =
    List.filter
      (fun name -> not (List.mem name documented))
      Tono_frontend.Trait_vocab.known
  in
  let extra =
    List.filter
      (fun name -> not (Tono_frontend.Trait_vocab.is_known name))
      documented
  in
  Alcotest.(check (list string)) "every known trait is documented" [] missing;
  Alcotest.(check (list string)) "nothing documented is unknown" [] extra

(* The editor's construct vocabulary is the parser's, in both directions: a
   reserved or positional word with no doc would hover blank and complete
   without prose, and a documented word the parser does not read would be
   advertised as syntax. The ext library block has its own contract in
   analysis_ext_test; this one covers everything else. *)
let construct_docs_cover_the_parser_vocabulary () =
  let documented = List.map fst Tono_lsp_lib.Hover_docs.construct_docs in
  let missing =
    List.filter
      (fun w -> not (List.mem w documented))
      Tono_frontend.Syntax_vocab.constructs
  in
  let extra =
    List.filter
      (fun w -> not (Tono_frontend.Syntax_vocab.is_construct w))
      documented
  in
  Alcotest.(check (list string))
    "every construct the parser reads is documented" [] missing;
  Alcotest.(check (list string))
    "nothing documented is outside the parser's vocabulary" [] extra;
  List.iter
    (fun w ->
      Alcotest.(check bool)
        ("construct_doc reaches " ^ w)
        true
        (Option.is_some (Tono_lsp_lib.Hover_docs.construct_doc w)))
    Tono_frontend.Syntax_vocab.constructs

(* Every primitive the lexer recognizes has a wire note under the cursor. *)
let primitive_docs_cover_the_lexer () =
  let missing =
    List.filter
      (fun p -> Option.is_none (Tono_lsp_lib.Hover_docs.primitive_doc p))
      Tono_frontend.Lexer.prims
  in
  Alcotest.(check (list string)) "every primitive is documented" [] missing

let () =
  Alcotest.run "analysis"
    [
      ( "vocabulary",
        [
          Alcotest.test_case "trait docs cover the compiler" `Quick
            trait_docs_cover_the_compiler_vocabulary;
          Alcotest.test_case "construct docs cover the parser" `Quick
            construct_docs_cover_the_parser_vocabulary;
          Alcotest.test_case "primitive docs cover the lexer" `Quick
            primitive_docs_cover_the_lexer;
        ] );
      ( "diagnostics",
        [
          Alcotest.test_case "clean" `Quick diagnostics_clean;
          Alcotest.test_case "error" `Quick diagnostics_error;
          Alcotest.test_case "unknown trait warns" `Quick
            diagnostics_unknown_trait;
        ] );
      ( "navigation",
        [
          Alcotest.test_case "definition resolves" `Quick definition_resolves;
          Alcotest.test_case "definition miss" `Quick definition_miss;
          Alcotest.test_case "hover decl" `Quick hover_decl;
          Alcotest.test_case "completions" `Quick completions_list;
          Alcotest.test_case "completion keywords" `Quick
            completions_offer_declaration_keywords;
          Alcotest.test_case "offset roundtrip" `Quick offset_roundtrip;
        ] );
      ( "workspace",
        [
          Alcotest.test_case "document symbols" `Quick document_symbols_outline;
          Alcotest.test_case "references" `Quick
            references_include_and_exclude_decl;
          Alcotest.test_case "rename" `Quick rename_edits_every_occurrence;
          Alcotest.test_case "formatting" `Quick formatting_uses_the_printer;
          Alcotest.test_case "formatting parse error" `Quick
            formatting_declines_on_parse_error;
        ] );
      ( "utf16",
        [
          Alcotest.test_case "offset of position" `Quick
            utf16_offset_of_position;
          Alcotest.test_case "hover range" `Quick utf16_hover_range;
          Alcotest.test_case "rename range" `Quick utf16_rename_range;
          Alcotest.test_case "token end" `Quick hover_at_token_end;
          Alcotest.test_case "formatting identity" `Quick
            formatting_identity_is_no_edit;
        ] );
      ( "hover content",
        [
          Alcotest.test_case "markdown declaration" `Quick hover_markdown_decl;
          Alcotest.test_case "member" `Quick hover_member;
          Alcotest.test_case "type ref shows target" `Quick
            hover_type_ref_shows_target_decl;
          Alcotest.test_case "keyword" `Quick hover_keyword;
          Alcotest.test_case "ext no longer lists a lifecycle" `Quick
            hover_ext_word_no_longer_lists_a_lifecycle;
          Alcotest.test_case "primitive" `Quick hover_primitive;
          Alcotest.test_case "nullable marker" `Quick hover_nullable_marker;
          Alcotest.test_case "trait contract" `Quick hover_trait_contract;
          Alcotest.test_case "value source" `Quick hover_value_source;
          Alcotest.test_case "str catalog" `Quick hover_str_catalog;
          Alcotest.test_case "match keyword" `Quick hover_match_keyword;
          Alcotest.test_case "field selection table" `Quick
            hover_field_shows_its_selection_table;
          Alcotest.test_case "entry operation" `Quick hover_entry_operation;
          Alcotest.test_case "raw marker" `Quick hover_raw_word;
        ] );
      ( "completion context",
        [
          Alcotest.test_case "after @" `Quick completion_after_at;
          Alcotest.test_case "no hook slots offered" `Quick
            completion_no_hook_slots_offered;
          Alcotest.test_case "type position" `Quick completion_type_position;
          Alcotest.test_case "op input" `Quick
            completion_op_input_is_a_type_position;
          Alcotest.test_case "value sources" `Quick
            completion_after_at_offers_value_sources;
          Alcotest.test_case "catalog separator" `Quick
            completion_after_catalog_separator;
          Alcotest.test_case "ext kinds" `Quick
            completion_after_ext_offers_kinds;
          Alcotest.test_case "ext impl operations" `Quick
            completion_after_ext_impl_offers_entry_ops;
          Alcotest.test_case "field reference" `Quick
            completion_after_dot_offers_sibling_fields;
        ] );
    ]
