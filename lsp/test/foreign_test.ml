(* Completion inside [#(...)] from an index handed in by a lookup: the
   index as read from its JSON, the key that decides whether it is served,
   where in the grammar the cursor is, what each position offers per
   language, the language-block hover, and the cost of a query. *)

open Lsp.Types
module Analysis = Tono_lsp_lib.Analysis
module AF = Tono_lsp_lib.Analysis_foreign
module AE = Tono_lsp_lib.Analysis_ext
module FI = Tono_lsp_lib.Foreign_index
module Lexer = Tono_frontend.Lexer

let contains s sub =
  let ls = String.length s and lsub = String.length sub in
  let rec go i = i + lsub <= ls && (String.sub s i lsub = sub || go (i + 1)) in
  lsub = 0 || go 0

(* --- the indexes a project would have on disk, one per language --- *)

let index_json ?(format_version = 1) ~lang ~package ~version ?note symbols =
  Printf.sprintf
    {|{"tono_index_version":%d,"key":{"ext":"gearbox","lang":"%s","package":"%s","version":"%s","lockfile":{"path":"/p/lock","digest":"none"},"format":1}%s,"symbols":[%s]}|}
    format_version lang package version
    (match note with Some n -> Printf.sprintf {|,"note":"%s"|} n | None -> "")
    (String.concat "," symbols)

let sym ?(members = "") name kind sigs =
  Printf.sprintf
    {|{"name":"%s","kind":"%s","signatures":[%s],"doc":"","members":[%s]}|} name
    kind sigs members

let mem ?(static = false) name kind sigs =
  Printf.sprintf {|{"name":"%s","kind":"%s","static":%b,"signatures":[%s]}|}
    name kind static sigs

let go_index =
  index_json ~lang:"go" ~package:"example.test/gearbox" ~version:"v0.0.0"
    [
      sym "Dial" "interface" {|"Dial[T any]"|}
        ~members:
          (String.concat ","
             [
               mem "Read" "method" {|"func(ctx context.Context) (T, error)"|};
               mem "Value" "method" {|"func() T"|};
             ]);
      sym "Fast" "const" {|"Mode = 1"|};
      sym "Mode" "type" "";
      sym "Open" "function" {|"func[T any](value T) (Dial[T], error)"|};
      sym "Options" "struct" "" ~members:(mem "Precision" "field" {|"int"|});
      sym "WithPrecision" "function" {|"func(p int) Option"|};
    ]

let ts_index =
  index_json ~lang:"ts" ~package:"@example/gearbox" ~version:"0.0.0"
    [
      sym "Dial" "class" {|"(value: number): Dial"|}
        ~members:
          (String.concat ","
             [
               mem ~static:true "create" "method" {|"(value: number): Dial"|};
               mem "read" "method" {|"(): number"|};
             ]);
      sym "Options" "interface" ""
        ~members:(mem "precision" "field" {|"number"|});
      sym "Size" "type" {|"\"s\" | \"m\""|};
      sym "build" "function"
        {|"(name: string): Dial","(name: string, size: number): Dial"|};
      sym "util" "namespace" ""
        ~members:(mem ~static:true "pad" "function" {|"(s: string): string"|});
    ]

let rust_index =
  index_json ~lang:"rust" ~package:"gearbox" ~version:"0.0.0"
    ~note:"parsed from source: what a macro produces is not indexed"
    [
      sym "Dial" "struct" {|"Dial<T>"|}
        ~members:
          (String.concat ","
             [
               mem ~static:true "open" "method" {|"fn open(value: T) -> Self"|};
               mem "read" "method" {|"fn read(&self) -> T"|};
             ]);
      sym "Options" "struct" "";
      sym "Run" "trait" "" ~members:(mem "run" "method" {|"fn run(&self)"|});
      sym "open" "function" {|"fn open(value: f64) -> Dial<f64>"|};
      sym "sub" "namespace" "";
      sym "sub::Gear" "struct" "";
      sym "sub::gear_fn" "function" {|"fn gear_fn()"|};
      sym "sub::deep::x" "const" {|"u8"|};
    ]

let ready text =
  match FI.of_string text with Ok t -> FI.Ready t | Error e -> Alcotest.fail e

let lookup ~ext ~lang =
  if ext <> "gearbox" then FI.Missing ("no ext named " ^ ext)
  else
    match lang with
    | "go" -> ready go_index
    | "ts" -> ready ts_index
    | "rust" -> ready rust_index
    | other -> FI.Missing (other ^ " not built")

(* --- the source, with every position a spelling can take --- *)

let src =
  {|ext gearbox {
  go { #(example.test/gearbox) }
  ts { #(@example/gearbox) }
  rust { #(gearbox) }

  struct options {
    ts { #(Options) }
    rust { #(Options) precision: #(Option<u8>) }
  }

  struct dial {
    go { #(Dial[float64]) }
    ts { #(Dial<number>) }
    rust { #(Box<dyn Dial<f64>>) }

    op read(): float {
      go { call: #(Read)(#(ctx context.Context)).#(Value)() }
      ts { call: #(read)() }
      rust { call: #(read)() }
    }
  }

  op open(value: float, opts: options): dial {
    go { call: #(Open[float64])(value, #(WithPrecision)(opts: #(int))) }
    ts { call: #(new Dial)(value) yields: (d: #(Dial<number>)) }
    rust { call: #(Dial::open)(value, opts { precision: precision }: #(&Options)) }
  }

  op close(): float {
    go { call: #(gearbox.Close)() }
    ts { call: #(Dial.create)() }
    rust { call: #(sub::gear_fn)() }
  }
}

pub struct parse_error {
  go { #(ErrParse) message: #(Error()) }
}
|}

(* The position [skip] characters after the first occurrence of [needle]:
   a cursor placed inside a spelling by naming its start. *)
let cursor ?(skip = 2) (src : string) (needle : string) : Position.t =
  let lines = String.split_on_char '\n' src in
  let rec find line = function
    | [] -> Alcotest.fail ("not in source: " ^ needle)
    | l :: rest -> (
        let ll = String.length l and ln = String.length needle in
        let rec col i =
          if i + ln > ll then None
          else if String.sub l i ln = needle then Some i
          else col (i + 1)
        in
        match col 0 with
        | Some c -> Position.create ~line ~character:(c + skip)
        | None -> find (line + 1) rest)
  in
  find 0 lines

let rec show_position = function
  | FI.Call_head { after_new } ->
      if after_new then "call-head(new)" else "call-head"
  | FI.Type_pos -> "type"
  | FI.Function_pos -> "function"
  | FI.Path -> "path"
  | FI.Member { head; base } ->
      Printf.sprintf "member(%s)<%s" head (show_position base)

let site (src : string) (p : Position.t) : string =
  let off = Analysis.offset_of_position src p in
  match AF.site_at ~text:src ~toks:(fst (Lexer.tokenize src)) off with
  | None -> "none"
  | Some s ->
      Printf.sprintf "%s/%s %s [%s]" s.ext s.lang (show_position s.position)
        s.prefix

let labels ?(foreign = lookup) (src : string) (p : Position.t) : string list =
  let file = Analysis.parse src in
  List.map
    (fun (c : CompletionItem.t) -> c.label)
    (Analysis.completions ~foreign ~text:src ~file p)

let hover ?(foreign = lookup) (src : string) (p : Position.t) : string option =
  let file = Analysis.parse src in
  match Analysis.hover_at ~foreign ~markdown:false ~text:src ~file p with
  | None -> None
  | Some h -> (
      match h.Hover.contents with
      | `MarkupContent mc -> Some mc.MarkupContent.value
      | _ -> None)

let check_str = Alcotest.(check string)
let check_strs = Alcotest.(check (list string))

(* --- the index as read --- *)

let index_reads_every_kind () =
  match FI.of_string ts_index with
  | Error e -> Alcotest.fail e
  | Ok t ->
      check_str "package" "@example/gearbox" t.key.package;
      check_str "version" "0.0.0" t.key.version;
      Alcotest.(check int) "format" 1 t.key.format;
      Alcotest.(check int) "symbols" 5 (List.length t.symbols);
      let dial = Option.get (FI.symbol t "Dial") in
      Alcotest.(check bool) "class" true (dial.kind = FI.Class);
      Alcotest.(check bool)
        "static create" true
        (List.exists
           (fun (m : FI.member) -> m.mname = "create" && m.static)
           dial.members);
      let build = Option.get (FI.symbol t "build") in
      Alcotest.(check int)
        "overloads on one symbol" 2
        (List.length build.signatures);
      Alcotest.(check bool) "no note" true (t.note = None)

let index_of_another_format_is_refused_and_an_unknown_kind_kept () =
  (match
     FI.of_string
       (index_json ~format_version:2 ~lang:"go" ~package:"p" ~version:"1" [])
   with
  | Error e ->
      Alcotest.(check bool) "says the format" true (contains e "index format 2")
  | Ok _ -> Alcotest.fail "a later format must not be read");
  (match
     FI.of_string
       (index_json ~lang:"go" ~package:"p" ~version:"1" [ sym "X" "widget" "" ])
   with
  | Ok t ->
      Alcotest.(check bool)
        "kept as other" true
        ((List.hd t.symbols).kind = FI.Other)
  | Error e -> Alcotest.fail e);
  (match FI.of_string "{not json" with
  | Error e -> Alcotest.(check bool) "unreadable" true (contains e "unreadable")
  | Ok _ -> Alcotest.fail "garbage read as an index");
  match FI.of_string {|{"tono_index_version":1,"symbols":[]}|} with
  | Error e -> Alcotest.(check bool) "no key" true (contains e "unreadable")
  | Ok _ -> Alcotest.fail "an index without a key read"

(* --- the key --- *)

let fnv_matches_the_builder () =
  check_str "empty" "cbf29ce484222325" (FI.fnv1a64_hex "");
  check_str "a" "af63dc4c8601ec8c" (FI.fnv1a64_hex "a");
  check_str "foobar" "85944171f73967e8" (FI.fnv1a64_hex "foobar");
  check_str "braces" "08f44b07b5901a25" (FI.fnv1a64_hex "{}")

let key_must_match_on_every_field () =
  let t =
    match FI.of_string go_index with Ok t -> t | Error e -> Alcotest.fail e
  in
  let expected ?(ext = "gearbox") ?(lang = "go")
      ?(package = "example.test/gearbox") ?(version = "v0.0.0")
      ?(path = "/p/lock") ?lockfile () =
    FI.expected_key ~ext ~lang ~package ~version ~lockfile_path:path ~lockfile
  in
  Alcotest.(check bool) "matches" true (FI.key_matches t (expected ()));
  Alcotest.(check bool)
    "ext" false
    (FI.key_matches t (expected ~ext:"other" ()));
  Alcotest.(check bool) "lang" false (FI.key_matches t (expected ~lang:"ts" ()));
  Alcotest.(check bool)
    "package" false
    (FI.key_matches t (expected ~package:"x" ()));
  Alcotest.(check bool)
    "version" false
    (FI.key_matches t (expected ~version:"v0.0.1" ()));
  Alcotest.(check bool)
    "lockfile path" false
    (FI.key_matches t (expected ~path:"/q" ()));
  Alcotest.(check bool)
    "lockfile appeared" false
    (FI.key_matches t (expected ~lockfile:"foobar" ()));
  check_str "digest of a present lockfile" "85944171f73967e8"
    (expected ~lockfile:"foobar" ()).lockfile_digest

(* --- where the cursor is --- *)

let frames_carry_the_names () =
  let toks = fst (Lexer.tokenize src) in
  let at needle =
    AE.state_at toks
      (Analysis.offset_of_position src (cursor ~skip:0 src needle))
  in
  let show = function
    | AE.Ext n -> "ext " ^ n
    | AE.Struct -> "struct"
    | AE.Op -> "op"
    | AE.Lang l -> "lang " ^ l
    | AE.Block l -> "block " ^ l
    | AE.Other -> "other"
  in
  let stack needle = String.concat " < " (List.map show (at needle).stack) in
  check_str "header block" "block go < ext gearbox" (stack "#(example.test");
  check_str "struct block" "block rust < struct < ext gearbox"
    (stack "#(Box<dyn");
  check_str "method body" "lang ts < op < struct < ext gearbox"
    (stack "#(read)() }");
  check_str "op body" "lang rust < op < ext gearbox" (stack "#(Dial::open)");
  check_str "error struct" "other < other" (stack "#(ErrParse");
  Alcotest.(check (option string))
    "ext of" (Some "gearbox")
    (AE.ext_of (at "#(Dial::open)").stack);
  Alcotest.(check (option string))
    "no ext" None
    (AE.ext_of (at "#(ErrParse").stack)

let every_position_is_classified () =
  let expect needle ?skip expected =
    check_str needle expected (site src (cursor ?skip src needle))
  in
  expect "#(Open[float64])" ~skip:6 "gearbox/go call-head [Open]";
  expect "#(new Dial)" ~skip:10 "gearbox/ts call-head(new) [new Dial]";
  expect "#(Dial::open)" ~skip:8 "gearbox/rust member(Dial)<call-head [Dial::]";
  expect "#(Dial.create)" ~skip:7 "gearbox/ts member(Dial)<call-head [Dial.]";
  expect "#(sub::gear_fn)" ~skip:7 "gearbox/rust member(sub)<call-head [sub::]";
  expect "#(gearbox.Close)" ~skip:10
    "gearbox/go member(gearbox)<call-head [gearbox.]";
  expect "#(Dial[float64])" "gearbox/go type []";
  expect "#(Box<dyn Dial<f64>>)" "gearbox/rust type []";
  expect "#(Options) precision" "gearbox/rust type []";
  expect "#(Option<u8>)" "gearbox/rust type []";
  expect "#(int)" "gearbox/go type []";
  expect "#(Dial<number>))" ~skip:14 "gearbox/ts type [Dial<number>]";
  expect "#(&Options)" "gearbox/rust type []";
  expect "#(Value)" "gearbox/go function []";
  expect "#(ctx context.Context)" "gearbox/go function []";
  expect "#(WithPrecision)" "gearbox/go function []";
  expect "#(example.test/gearbox)" ~skip:14 "gearbox/go path [example.test]";
  expect "#(ErrParse)" "none";
  expect "#(Error())" "none";
  (* On the hash, and right after the closing paren: outside. *)
  expect "#(Open[float64])" ~skip:0 "none";
  expect "#(Open[float64])" ~skip:16 "none";
  (* Just before the closing paren is still inside. *)
  expect "#(Open[float64])" ~skip:15 "gearbox/go call-head [Open[float64]]"

let an_unterminated_spelling_still_has_a_site () =
  let partial =
    "ext gearbox {\n\
    \  go { #(example.test/gearbox) }\n\
    \  op open(): float {\n\
    \    go { call: #(Lo"
  in
  let p =
    Position.create ~line:3 ~character:(String.length "    go { call: #(Lo")
  in
  check_str "at the end" "gearbox/go call-head [Lo]" (site partial p);
  let p =
    Position.create ~line:3 ~character:(String.length "    go { call: #(")
  in
  check_str "empty prefix" "gearbox/go call-head []" (site partial p);
  check_strs "offers the callees"
    [ "Open"; "Options"; "WithPrecision" ]
    (labels partial p)

(* --- what each position offers --- *)

let call_head_offers_functions_and_classes () =
  check_strs "go"
    [ "Open"; "Options"; "WithPrecision" ]
    (labels src (cursor src "#(Open[float64])"));
  check_strs "ts"
    [ "Dial"; "build"; "util" ]
    (labels src (cursor src "#(new Dial)"));
  check_strs "ts after new" [ "Dial" ]
    (labels src (cursor ~skip:6 src "#(new Dial)"));
  check_strs "rust"
    [ "Dial"; "Options"; "open"; "sub"; "sub::Gear"; "sub::gear_fn" ]
    (labels src (cursor src "#(Dial::open)"))

let members_follow_the_head () =
  check_strs "ts static and instance" [ "create"; "read" ]
    (labels src (cursor ~skip:7 src "#(Dial.create)"));
  check_strs "rust associated" [ "open"; "read" ]
    (labels src (cursor ~skip:8 src "#(Dial::open)"));
  check_strs "rust module path" [ "Gear"; "gear_fn" ]
    (labels src (cursor ~skip:7 src "#(sub::gear_fn)"));
  (* The Go package selector names the top level again. *)
  check_strs "go selector"
    [ "Open"; "Options"; "WithPrecision" ]
    (labels src (cursor ~skip:10 src "#(gearbox.Close)"));
  let unknown =
    "ext gearbox {\n\
    \  ts { #(@example/gearbox) }\n\
    \  op c(): float {\n\
    \    ts { call: #(Nope."
  in
  check_strs "unknown head" []
    (labels unknown (cursor ~skip:7 unknown "#(Nope."))

let type_position_offers_types () =
  check_strs "go storage"
    [ "Dial"; "Mode"; "Options" ]
    (labels src (cursor src "#(Dial[float64])"));
  check_strs "rust field"
    [ "Dial"; "Options"; "Run"; "sub"; "sub::Gear" ]
    (labels src (cursor src "#(Option<u8>)"));
  check_strs "go argument"
    [ "Dial"; "Mode"; "Options" ]
    (labels src (cursor src "#(int)"));
  check_strs "ts yields"
    [ "Dial"; "Options"; "Size"; "util" ]
    (labels src (cursor src "#(Dial<number>))"))

let function_position_offers_functions () =
  check_strs "chained"
    [ "Open"; "WithPrecision" ]
    (labels src (cursor src "#(Value)"));
  check_strs "nested"
    [ "Open"; "WithPrecision" ]
    (labels src (cursor src "#(WithPrecision)"));
  check_strs "bare"
    [ "Open"; "WithPrecision" ]
    (labels src (cursor src "#(ctx context.Context)"))

let details_carry_signatures_and_docs () =
  let file = Analysis.parse src in
  let items =
    Analysis.completions ~foreign:lookup ~text:src ~file
      (cursor src "#(new Dial)")
  in
  let detail label =
    Option.get
      (List.find (fun (c : CompletionItem.t) -> c.label = label) items).detail
  in
  check_str "overloads" "(name: string): Dial (+1 overloads)" (detail "build");
  check_str "constructor" "(value: number): Dial" (detail "Dial");
  check_str "namespace" "namespace" (detail "util");
  let members =
    Analysis.completions ~foreign:lookup ~text:src ~file
      (cursor ~skip:7 src "#(Dial.create)")
  in
  let create =
    List.find (fun (c : CompletionItem.t) -> c.label = "create") members
  in
  check_str "static member" "static (value: number): Dial"
    (Option.get create.detail)

let nothing_without_an_index_and_nothing_of_the_block_words () =
  check_strs "path" [] (labels src (cursor src "#(example.test/gearbox)"));
  check_strs "no ext" [] (labels src (cursor src "#(ErrParse)"));
  check_strs "missing" []
    (labels
       ~foreign:(fun ~ext:_ ~lang:_ -> FI.Missing "not built")
       src
       (cursor src "#(Open[float64])"));
  check_strs "default lookup" []
    (labels
       ~foreign:(fun ~ext ~lang ->
         ignore (ext, lang);
         FI.Missing "none")
       src
       (cursor src "#(Open[float64])"));
  let file = Analysis.parse src in
  check_strs "no lookup at all" []
    (List.map
       (fun (c : CompletionItem.t) -> c.label)
       (Analysis.completions ~text:src ~file (cursor src "#(Open[float64])")));
  (* Outside the spelling the block words are still offered. *)
  let inside_block = "ext gearbox {\n  op open(): float {\n    go { " in
  let p = Position.create ~line:2 ~character:(String.length "    go { ") in
  check_strs "block words"
    [ "call"; "yields"; "returns" ]
    (labels inside_block p)

(* --- the language-block hover --- *)

let the_lang_word_shows_the_index_status () =
  let h needle = Option.get (hover src (cursor ~skip:0 src needle)) in
  Alcotest.(check bool)
    "go count" true
    (contains (h "go { #(example.test")
       "6 symbols of example.test/gearbox v0.0.0");
  Alcotest.(check bool)
    "rust note" true
    (contains (h "rust { #(Box") "what a macro produces is not indexed");
  Alcotest.(check bool)
    "op body" true
    (contains (h "ts { call: #(new") "5 symbols of @example/gearbox 0.0.0");
  let missing =
    hover
      ~foreign:(fun ~ext:_ ~lang:_ -> FI.Missing "not built yet")
      src
      (cursor ~skip:0 src "ts { #(@example")
  in
  Alcotest.(check bool)
    "reason" true
    (contains (Option.get missing) "no completion inside #(...): not built yet");
  Alcotest.(check bool)
    "error struct has no ext" true
    (match hover src (cursor ~skip:0 src "go { #(ErrParse") with
    | None -> true
    | Some v -> not (contains v "symbols of"));
  Alcotest.(check bool)
    "the word itself" true
    (contains (h "go { #(example.test") "go")

(* --- the cost of a query --- *)

let a_large_index_answers_under_the_budget () =
  let symbols =
    List.init 5000 (fun i ->
        let name = Printf.sprintf "S%04d" i in
        sym name
          (if i mod 2 = 0 then "class" else "function")
          (Printf.sprintf {|"(a: %s, b: number): %s"|} name name)
          ~members:
            (String.concat ","
               [
                 mem ~static:true "create" "method" {|"(): void"|};
                 mem "read" "method" {|"(): number"|};
                 mem "value" "field" {|"number"|};
               ]))
  in
  let text =
    index_json ~lang:"ts" ~package:"@example/big" ~version:"1" symbols
  in
  let t0 = Sys.time () in
  let index =
    match FI.of_string text with Ok t -> t | Error e -> Alcotest.fail e
  in
  let parse = Sys.time () -. t0 in
  let positions =
    [
      FI.Call_head { after_new = false };
      FI.Member { head = "S2500"; base = FI.Type_pos };
      FI.Type_pos;
      FI.Function_pos;
    ]
  in
  let times =
    List.init 200 (fun i ->
        let p = List.nth positions (i mod 4) in
        let t0 = Sys.time () in
        ignore (FI.items index p);
        Sys.time () -. t0)
  in
  let sorted = List.sort compare times in
  let p50 = List.nth sorted 100 and worst = List.nth sorted 199 in
  Printf.printf
    "index of %d symbols (%d bytes): parse %.1fms, query p50 %.2fms max %.2fms\n\
     %!"
    (List.length index.symbols)
    (String.length text) (parse *. 1000.) (p50 *. 1000.) (worst *. 1000.);
  Alcotest.(check bool) "parse under a second" true (parse < 1.0);
  Alcotest.(check bool) "every query under 100ms" true (worst < 0.1)

let () =
  Alcotest.run "foreign"
    [
      ( "index",
        [
          Alcotest.test_case "reads every kind" `Quick index_reads_every_kind;
          Alcotest.test_case "format and unknown kind" `Quick
            index_of_another_format_is_refused_and_an_unknown_kind_kept;
          Alcotest.test_case "fnv" `Quick fnv_matches_the_builder;
          Alcotest.test_case "key" `Quick key_must_match_on_every_field;
        ] );
      ( "site",
        [
          Alcotest.test_case "frames carry names" `Quick frames_carry_the_names;
          Alcotest.test_case "every position" `Quick
            every_position_is_classified;
          Alcotest.test_case "unterminated" `Quick
            an_unterminated_spelling_still_has_a_site;
        ] );
      ( "items",
        [
          Alcotest.test_case "call head" `Quick
            call_head_offers_functions_and_classes;
          Alcotest.test_case "members" `Quick members_follow_the_head;
          Alcotest.test_case "type position" `Quick type_position_offers_types;
          Alcotest.test_case "function position" `Quick
            function_position_offers_functions;
          Alcotest.test_case "details" `Quick details_carry_signatures_and_docs;
          Alcotest.test_case "nothing without an index" `Quick
            nothing_without_an_index_and_nothing_of_the_block_words;
        ] );
      ( "hover",
        [
          Alcotest.test_case "lang word" `Quick
            the_lang_word_shows_the_index_status;
        ] );
      ( "latency",
        [
          Alcotest.test_case "large index" `Quick
            a_large_index_answers_under_the_budget;
        ] );
    ]
