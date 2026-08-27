(* The pure side of the binding check: which pairs a file declares, what
   dirties each of them, and how the check's report lands as diagnostics. *)

open Lsp.Types
module BC = Tono_lsp_lib.Binding_check
module Analysis = Tono_lsp_lib.Analysis
module Span = Tono_frontend.Span

let source =
  {|ext gearbox {
  go { #(example.test/gearbox) }
  ts { #(@example/gearbox) }
  rust { #(gearbox) }

  struct dial {
    go { #(Dial[float64]) }
    ts { #(Dial<number>) }
    rust { #(Dial<f64>) }

    op read(): float {
      go { call: #(Read)(#(ctx context.Context)) }
      ts { call: #(read)() }
      rust { call: #(read)() }
    }
  }

  op open(value: float): dial {
    go { call: #(Open[float64])(value) }
    ts { call: #(new Dial)(value) }
    rust { call: #(Dial::open)(value) }
  }
}
|}

let manifest =
  {|[project]
name = "demo"

[target.go]
out = "dist/go"
package = "example.com/demo"

[target.typescript]
out = "dist/ts"

[ext.gearbox]
go = "v1.2.0"
typescript = "1.2.0"
rust = "1.2"
|}

(* [s] with the first occurrence of [a] replaced by [b]; the fixture edits
   are all single-site. *)
let replace ~a ~b s =
  let n = String.length s and m = String.length a in
  let rec find i =
    if i + m > n then None
    else if String.sub s i m = a then Some i
    else find (i + 1)
  in
  match find 0 with
  | Some i -> String.sub s 0 i ^ b ^ String.sub s (i + m) (n - i - m)
  | None -> failwith ("fixture has no " ^ a)

let parse text =
  let file, diags = Tono_frontend.Parser.parse text in
  Alcotest.(check int) "fixture parses clean" 0 (List.length diags);
  file

let pair ext lang = { BC.ext; lang }
let go = pair "gearbox" "go"
let ts = pair "gearbox" "ts"
let rust = pair "gearbox" "rust"

let pairs_and_regions () =
  let file = parse source in
  Alcotest.(check (list (pair string string)))
    "one pair per language, in order of appearance"
    [ ("gearbox", "go"); ("gearbox", "ts"); ("gearbox", "rust") ]
    (List.map (fun (p : BC.pair) -> (p.ext, p.lang)) (BC.pairs file));
  List.iter
    (fun (p, spans) ->
      Alcotest.(check int)
        ("regions of " ^ p.BC.lang ^ ": path, storage, method, op")
        4 (List.length spans))
    (BC.regions file);
  (* Every region is the language's own text: it starts with the language
     word and ends at its closing brace or at the path. *)
  List.iter
    (fun ((p : BC.pair), spans) ->
      List.iter
        (fun (s : Span.span) ->
          let t =
            String.sub source s.start.offset (s.finish.offset - s.start.offset)
          in
          let word = match p.lang with "ts" -> "ts" | l -> l in
          Alcotest.(check bool)
            ("region of " ^ p.lang ^ " starts with it")
            true
            (String.length t > String.length word
            && String.sub t 0 (String.length word) = word))
        spans)
    (BC.regions file);
  (* The text with every region cut out is what every language crosses. *)
  let logical =
    BC.without ~text:source (List.concat_map snd (BC.regions file))
  in
  Alcotest.(check bool)
    "no language block survives" false
    (List.exists
       (fun w ->
         let n = String.length w in
         let rec has i =
           i + n <= String.length logical
           && (String.sub logical i n = w || has (i + 1))
         in
         has 0)
       [ "call:"; "#(" ]);
  Alcotest.(check bool)
    "the op signature survives" true
    (let w = "op open(value: float): dial" in
     let n = String.length w in
     let rec has i =
       i + n <= String.length logical
       && (String.sub logical i n = w || has (i + 1))
     in
     has 0)

let keys text manifest =
  let file = parse text in
  List.map (fun p -> BC.key ~text ~manifest file p) [ go; ts; rust ]

let editing_one_block_dirties_one_pair () =
  let base = keys source (Some manifest) in
  let go_edit =
    keys
      (replace ~a:"#(Read)(#(ctx context.Context))" ~b:"#(Read)()" source)
      (Some manifest)
  in
  Alcotest.(check (list bool))
    "only the go key changes" [ true; false; false ]
    (List.map2 (fun a b -> a <> b) base go_edit);
  let sig_edit =
    keys
      (replace ~a:"open(value: float)" ~b:"open(value: i64)" source)
      (Some manifest)
  in
  Alcotest.(check (list bool))
    "an op signature changes every key" [ true; true; true ]
    (List.map2 (fun a b -> a <> b) base sig_edit);
  let comment_edit =
    keys
      (replace ~a:"    ts { call: #(new Dial)(value) }"
         ~b:"    ts { call: #(new Dial)(value) } // dial it" source)
      (Some manifest)
  in
  Alcotest.(check (list bool))
    "text outside every block is what all languages share" [ true; true; true ]
    (List.map2 (fun a b -> a <> b) base comment_edit)

let the_manifest_pin_is_part_of_the_key () =
  let base = keys source (Some manifest) in
  let bumped =
    keys source
      (Some (replace ~a:"go = \"v1.2.0\"" ~b:"go = \"v1.3.0\"" manifest))
  in
  Alcotest.(check (list bool))
    "the go pin dirties the go pair alone" [ true; false; false ]
    (List.map2 (fun a b -> a <> b) base bumped);
  let moved =
    keys source
      (Some (replace ~a:"out = \"dist/ts\"" ~b:"out = \"sdk/ts\"" manifest))
  in
  Alcotest.(check (list bool))
    "the typescript target's layout dirties the ts pair alone"
    [ false; true; false ]
    (List.map2 (fun a b -> a <> b) base moved);
  Alcotest.(check (list bool))
    "no manifest is a stable key of its own" [ false; false; false ]
    (List.map2 (fun a b -> a <> b) (keys source None) (keys source None));
  Alcotest.(check string)
    "a table is read up to the next one"
    "go = \"v1.2.0\"\ntypescript = \"1.2.0\"\nrust = \"1.2\"\n"
    (BC.section ~manifest "[ext.gearbox]");
  Alcotest.(check string)
    "an absent table is empty" ""
    (BC.section ~manifest "[ext.other]")

let spans_parse_as_the_check_prints_them () =
  let text = "ab\ncdef\ngh" in
  (match BC.span_of_string ~text "2:2-4" with
  | Some s ->
      Alcotest.(check (pair int int))
        "one-line span offsets" (4, 6)
        (s.start.offset, s.finish.offset)
  | None -> Alcotest.fail "one-line span");
  (match BC.span_of_string ~text "1:1-3:2" with
  | Some s ->
      Alcotest.(check (pair int int))
        "two-line span offsets" (0, 9)
        (s.start.offset, s.finish.offset)
  | None -> Alcotest.fail "two-line span");
  Alcotest.(check bool)
    "garbage is None" true
    (BC.span_of_string ~text "nope" = None
    && BC.span_of_string ~text "1:x-2" = None)

let finding_line =
  {|{"kind":"finding","code":"FX0001","span":"19:16-43","message":"go binding of op open in ext gearbox: too many arguments","site":{"ext":"gearbox","lang":"go","kind":"op","owner":null,"name":"open","span":"19:16-43"}}|}

let lines =
  [
    finding_line;
    {|{"kind":"unchecked","message":"go op stamp: no terms in ext gearbox"}|};
    {|{"kind":"checked","message":"go bindings of ext gearbox (go build)"}|};
    {|{"kind":"error","message":"checking the go bindings of ext gearbox needs go, which is not installed"}|};
    "this is not json";
    "";
  ]

let diagnostics text = BC.diagnostics_of_lines ~text ~file:(parse text) go lines

let report_lines_become_diagnostics () =
  let ds = diagnostics source in
  Alcotest.(check int) "checked and blank lines show nothing" 4 (List.length ds);
  let finding = List.nth ds 0 in
  Alcotest.(check bool)
    "a finding is an error carrying its code" true
    (finding.severity = Some DiagnosticSeverity.Error
    && finding.code = Some (`String "FX0001")
    && finding.source = Some "tono");
  (* line 19 col 16 is the call: spelling of op open, 1-based in the check,
     0-based in the editor. *)
  Alcotest.(check (pair int int))
    "the finding sits at its binding" (18, 15)
    (finding.range.start.line, finding.range.start.character);
  let note = List.nth ds 1 in
  Alcotest.(check bool)
    "unchecked is a note at the pair's path line" true
    (note.severity = Some DiagnosticSeverity.Information
    && note.range.start.line = 1
    && note.message
       = `String "not checked: go op stamp: no terms in ext gearbox");
  let failed = List.nth ds 2 in
  Alcotest.(check bool)
    "a check that could not run is a warning there" true
    (failed.severity = Some DiagnosticSeverity.Warning
    && failed.range.start.line = 1
    && failed.message
       = `String
           "not checked: checking the go bindings of ext gearbox needs go, \
            which is not installed");
  let garbage = List.nth ds 3 in
  Alcotest.(check bool)
    "an unreadable line is shown, never dropped" true
    (garbage.severity = Some DiagnosticSeverity.Warning
    && garbage.message
       = `String "binding check: unreadable report line: this is not json");
  Alcotest.(check (list (option string)))
    "each diagnostic renders back as the check printed it"
    [
      Some
        "19:16-43: error: FX0001: go binding of op open in ext gearbox: too \
         many arguments";
      Some "not checked: go op stamp: no terms in ext gearbox";
      Some
        "checking the go bindings of ext gearbox needs go, which is not \
         installed";
      None;
    ]
    (List.map BC.report_line ds)

let a_finding_follows_its_binding_across_edits () =
  (* Two lines inserted above the ext: the cached span points two lines
     short, the site does not. *)
  let shifted = "// one\n// two\n" ^ source in
  let finding = List.nth (diagnostics shifted) 0 in
  Alcotest.(check (pair int int))
    "re-located by site" (20, 15)
    (finding.range.start.line, finding.range.start.character);
  (* Without a site the span is taken as printed; without either, the
     anchor. *)
  let no_site =
    BC.diagnostics_of_lines ~text:shifted ~file:(parse shifted) go
      [
        {|{"kind":"finding","code":"FX0001","span":"19:16-43","message":"m"}|};
        {|{"kind":"finding","code":"FX0001","span":"?","message":"m"}|};
      ]
  in
  Alcotest.(check (list int))
    "span as printed, then the anchor" [ 18; 3 ]
    (List.map (fun (d : Diagnostic.t) -> d.range.start.line) no_site);
  (* A pair whose ext is gone anchors at the file's start. *)
  let gone =
    BC.diagnostics_of_lines ~text:"struct s { a: i64 }"
      ~file:(parse "struct s { a: i64 }")
      go
      [ {|{"kind":"unchecked","message":"m"}|} ]
  in
  Alcotest.(check int) "no ext: file start" 0 (List.hd gone).range.start.line;
  Alcotest.(check bool)
    "an ext without that language anchors at its name" true
    (match
       BC.anchor (parse "ext solo {\n  go { #(x) }\n}") (pair "solo" "ts")
     with
    | Some s -> s.start.line = 1
    | None -> false)

let () =
  Alcotest.run "binding_check"
    [
      ( "pairs",
        [
          Alcotest.test_case "pairs and regions" `Quick pairs_and_regions;
          Alcotest.test_case "one block dirties one pair" `Quick
            editing_one_block_dirties_one_pair;
          Alcotest.test_case "manifest pin" `Quick
            the_manifest_pin_is_part_of_the_key;
        ] );
      ( "report",
        [
          Alcotest.test_case "span parsing" `Quick
            spans_parse_as_the_check_prints_them;
          Alcotest.test_case "lines to diagnostics" `Quick
            report_lines_become_diagnostics;
          Alcotest.test_case "finding follows its binding" `Quick
            a_finding_follows_its_binding_across_edits;
        ] );
    ]
