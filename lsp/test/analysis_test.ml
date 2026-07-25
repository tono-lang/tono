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

let hover_decl () =
  let file = Analysis.parse two_shapes in
  match Analysis.hover_at ~text:two_shapes ~file (pos 0 8) with
  | None -> Alcotest.fail "expected hover on a declaration name"
  | Some h -> (
      match h.Hover.contents with
      | `MarkupContent mc ->
          Alcotest.(check bool)
            "describes the struct" true
            (contains mc.MarkupContent.value "struct point")
      | _ -> Alcotest.fail "expected markup content")

let completions_list () =
  let file = Analysis.parse two_shapes in
  let labels =
    List.map
      (fun (c : CompletionItem.t) -> c.label)
      (Analysis.completions ~file)
  in
  Alcotest.(check bool) "declared shape point" true (List.mem "point" labels);
  Alcotest.(check bool) "declared shape edge" true (List.mem "edge" labels);
  Alcotest.(check bool) "primitive i64" true (List.mem "i64" labels)

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
  match Analysis.hover_at ~text:accented ~file (pos 0 20) with
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

let () =
  Alcotest.run "analysis"
    [
      ( "diagnostics",
        [
          Alcotest.test_case "clean" `Quick diagnostics_clean;
          Alcotest.test_case "error" `Quick diagnostics_error;
        ] );
      ( "navigation",
        [
          Alcotest.test_case "definition resolves" `Quick definition_resolves;
          Alcotest.test_case "definition miss" `Quick definition_miss;
          Alcotest.test_case "hover decl" `Quick hover_decl;
          Alcotest.test_case "completions" `Quick completions_list;
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
        ] );
    ]
