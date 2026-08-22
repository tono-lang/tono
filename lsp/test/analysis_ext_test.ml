(* The editor surface of the "ext" library block: hover on its contextual
   words, its outline, completion inside it and at a call site, and the
   contract that ties the documented vocabulary to the grammar's own list. *)

open Lsp.Types
module Analysis = Tono_lsp_lib.Analysis
module Hover_docs = Tono_lsp_lib.Hover_docs
module Vocab = Tono_frontend.Ext_lib_vocab

let contains s sub =
  let ls = String.length s and lsub = String.length sub in
  let rec go i = i + lsub <= ls && (String.sub s i lsub = sub || go (i + 1)) in
  lsub = 0 || go 0

let pos line character = Position.create ~line ~character

let hover_value (src : string) (p : Position.t) : string =
  let file = Analysis.parse src in
  match Analysis.hover_at ~markdown:false ~text:src ~file p with
  | None -> Alcotest.fail "expected a hover"
  | Some h -> (
      match h.Hover.contents with
      | `MarkupContent mc -> mc.MarkupContent.value
      | _ -> Alcotest.fail "expected markup content")

let no_hover (src : string) (p : Position.t) : bool =
  let file = Analysis.parse src in
  Option.is_none (Analysis.hover_at ~markdown:false ~text:src ~file p)

let completion_labels (src : string) (p : Position.t) : string list =
  let file = Analysis.parse src in
  List.map
    (fun (c : CompletionItem.t) -> c.label)
    (Analysis.completions ~text:src ~file p)

(* Line and column of the first occurrence of [needle] in [src], so a test
   points at a word rather than counting columns by hand. *)
let at (src : string) (needle : string) : Position.t =
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
        | Some c -> pos line (c + 1)
        | None -> find (line + 1) rest)
  in
  find 0 lines

(* One block exercising every construct: a handle with a method, a free
   extern with every language-block line, and a call site reading .request. *)
let lib_src =
  {|ext bus {
  go { #(github.com/example/bus) }
  rust { #(bus) }

  struct go_ack { ID: string, OK: bool }

  struct conn {
    op publish(topic: string): ack {
      go {
        call: #(Publish)(topic)
        yields: (a: go_ack)
        returns: ack { id: .a.ID, accepted: .a.OK }
      }
      rust {
        call: #(publish)(topic)
      }
    }
  }

  op connect(endpoint: string): conn {
    go { call: #(Connect)(endpoint) }
  }
}

struct ack { id: string, accepted: bool }
struct overloaded { code: string }

pub struct client {
  endpoint: string @arg
  link: conn = bus.connect(.endpoint)

  @http(method: "GET", path: "/ping")
  @header("Authorization", bus.connect(.request))
  op ping(): ack
}
|}

let hover_construct_words () =
  let check word expected =
    let v = hover_value lib_src (at lib_src word) in
    Alcotest.(check bool) ("hover on " ^ word) true (contains v expected)
  in
  check "ext bus" "third-party library";
  check "op connect" "foreign call";
  check "struct conn" "opaque handle";
  check "call:" "foreign callee";
  check "yields:" "position by position";
  check "returns:" "declared return type"

let hover_request_reference () =
  let v = hover_value lib_src (at lib_src ".request") in
  Alcotest.(check bool)
    "explains where it exists" true
    (contains v "already assembled" && contains v "@header/@body")

(* `http.request` is the transport shape, not the assembled-request
   reference: a dot after an identifier is a qualifier. *)
let hover_qualified_request_is_not_the_reference () =
  let src = "import tono.http\nstruct s { r: http.request }" in
  let v = hover_value src (pos 1 20) in
  Alcotest.(check bool)
    "resolves as a type reference" false
    (contains v "already assembled")

let hover_named_extern_and_handle () =
  let v = hover_value lib_src (at lib_src "connect(endpoint") in
  Alcotest.(check bool)
    "prints the extern as fmt does" true
    (contains v "op connect(endpoint: string): conn {");
  let v = hover_value lib_src (at lib_src "conn {") in
  Alcotest.(check bool)
    "prints the handle with its methods" true
    (contains v "struct conn {" && contains v "op publish");
  let v = hover_value lib_src (at lib_src "publish(topic") in
  Alcotest.(check bool)
    "a method hovers like a free op" true
    (contains v "op publish(topic: string): ack {")

(* A foreign struct field named like a language-block word is a field. *)
let hover_field_named_like_a_word_is_not_a_construct () =
  let src = "ext lib {\n  go { #(x) }\n  struct cfg { call: string }\n}" in
  Alcotest.(check bool)
    "no construct hover on a field" true
    (no_hover src (pos 2 16))

let ext_construct_doc_is_present () =
  match Hover_docs.construct_doc "ext" with
  | None -> Alcotest.fail "construct_doc \"ext\" must document the block"
  | Some d ->
      Alcotest.(check bool) "covers the library form" true (contains d "#(...)")

(* The documented vocabulary is the grammar's: every contextual word the
   parser recognizes hovers with prose, and nothing is documented that the
   grammar does not spell. *)
let ext_lib_docs_cover_the_grammar () =
  let grammar = Vocab.lang_fields @ [ Vocab.request_ref ] in
  (* Fixed, not merely complete: the documented set is this literal list,
     so a documented new word fails here rather than slipping in. *)
  Alcotest.(check (list string))
    "the documented ext vocabulary is exactly this"
    [ "call"; "yields"; "returns"; "request" ]
    (List.map fst Hover_docs.ext_lib_docs);
  let documented = List.map fst Hover_docs.ext_lib_docs in
  let missing = List.filter (fun w -> not (List.mem w documented)) grammar in
  let extra = List.filter (fun w -> not (List.mem w grammar)) documented in
  Alcotest.(check (list string)) "every grammar word is documented" [] missing;
  Alcotest.(check (list string)) "nothing documented is outside" [] extra;
  List.iter
    (fun w ->
      Alcotest.(check bool)
        ("construct_doc reaches " ^ w)
        true
        (Option.is_some (Hover_docs.construct_doc w)))
    grammar

(* The parser accepts exactly the vocabulary: every language-block word
   parses inside a block, and the diagnostic for a stray word enumerates the
   accepted ones from the same list. *)
let parser_accepts_the_vocabulary () =
  let block lines =
    "ext lib {\n  go { #(x) }\n  op f(a: string): string {\n    go {\n"
    ^ String.concat "\n" (List.map (fun l -> "      " ^ l) lines)
    ^ "\n    }\n  }\n}\n"
  in
  let diags src = snd (Tono_frontend.Parser.parse src) in
  Alcotest.(check int)
    "every field and marker parses" 0
    (List.length
       (diags
          (block
             [ "call: #(F)(a)"; "yields: (r: string)"; "returns: string { }" ])));
  match diags (block [ "call: #(F)(a)"; "bogus" ]) with
  | [] -> Alcotest.fail "a stray word must be diagnosed"
  | d :: _ -> (
      List.iter
        (fun w ->
          Alcotest.(check bool)
            ("diagnostic names " ^ w) true
            (contains d.Tono_frontend.Diagnostic.message w))
        Vocab.lang_fields;
      match diags "ext lib {\n  go { #(x) }\n  42\n}\n" with
      | [] -> Alcotest.fail "a stray token in the ext body must be diagnosed"
      | d :: _ ->
          List.iter
            (fun w ->
              Alcotest.(check bool)
                ("ext body diagnostic names " ^ w)
                true
                (contains d.Tono_frontend.Diagnostic.message w))
            [ "struct"; "op" ])

let outline_of_the_example () =
  let src = In_channel.with_open_bin "service.tono" In_channel.input_all in
  let file = Analysis.parse src in
  let syms = Analysis.document_symbols ~text:src ~file in
  let lib =
    List.find (fun (s : DocumentSymbol.t) -> s.name = "configlib") syms
  in
  let children = Option.value ~default:[] lib.DocumentSymbol.children in
  let names = List.map (fun (s : DocumentSymbol.t) -> s.name) children in
  Alcotest.(check (list string))
    "structs then the extern, in source order"
    [
      "go_config";
      "ts_config";
      "rust_config";
      "load(service: string, region: string): app_config";
    ]
    names;
  let load = List.nth children 3 in
  Alcotest.(check bool)
    "the extern is a method" true
    (load.DocumentSymbol.kind = SymbolKind.Method);
  let go_config = List.hd children in
  Alcotest.(check bool)
    "a foreign struct is a struct with its fields" true
    (go_config.DocumentSymbol.kind = SymbolKind.Struct
    && List.length (Option.value ~default:[] go_config.DocumentSymbol.children)
       = 4)

let outline_of_a_handle () =
  let file = Analysis.parse lib_src in
  let syms = Analysis.document_symbols ~text:lib_src ~file in
  let lib = List.hd syms in
  let children = Option.value ~default:[] lib.DocumentSymbol.children in
  let conn =
    List.find (fun (s : DocumentSymbol.t) -> s.name = "conn") children
  in
  Alcotest.(check bool)
    "a handle is a class" true
    (conn.DocumentSymbol.kind = SymbolKind.Class);
  let methods =
    Option.value ~default:[] conn.DocumentSymbol.children
    |> List.map (fun (s : DocumentSymbol.t) -> s.name)
  in
  Alcotest.(check (list string))
    "its methods are children"
    [ "publish(topic: string): ack" ]
    methods

let completion_in_a_language_block () =
  let src =
    "ext lib {\n  go { #(x) }\n  op f(a: string): string {\n    go {\n      "
  in
  let labels = completion_labels src (pos 4 6) in
  List.iter
    (fun w -> Alcotest.(check bool) ("offers " ^ w) true (List.mem w labels))
    Vocab.lang_fields;
  Alcotest.(check bool)
    "not the declaration list" false
    (List.mem "struct" labels || List.mem "i64" labels)

let completion_in_an_ext_body () =
  let src = "ext lib {\n  go { #(x) }\n  " in
  let labels = completion_labels src (pos 2 2) in
  Alcotest.(check bool)
    "offers the block words" true
    (List.mem "op" labels && List.mem "struct" labels);
  let src = "ext lib {\n  go { #(x) }\n  struct h {\n    " in
  let labels = completion_labels src (pos 3 4) in
  Alcotest.(check (list string)) "a handle body takes ops" [ "op" ] labels

(* A type position inside a language block still wants a type. *)
let completion_type_position_inside_a_block () =
  let src =
    "ext lib {\n\
    \  go { #(x) }\n\
    \  op f(a: string): string {\n\
    \    go {\n\
    \      returns: "
  in
  let labels = completion_labels src (pos 4 15) in
  Alcotest.(check bool) "types offered" true (List.mem "string" labels);
  Alcotest.(check bool) "not the block words" false (List.mem "call" labels)

let completion_after_ext_name_offers_its_externs () =
  let src = lib_src ^ "\npub struct other {\n  x: conn = bus." in
  let line = List.length (String.split_on_char '\n' src) - 1 in
  let labels = completion_labels src (pos line 16) in
  Alcotest.(check (list string))
    "the free externs, not the methods" [ "connect" ] labels

let completion_after_handle_field_offers_its_methods () =
  let src = lib_src ^ "\npub struct other {\n  link: conn\n  y: ack = link." in
  let line = List.length (String.split_on_char '\n' src) - 1 in
  let labels = completion_labels src (pos line 16) in
  Alcotest.(check (list string)) "the handle's methods" [ "publish" ] labels

(* A member sourced from a handle method (`= .link.`) reaches the same
   call-site lookup as `impl .link.`: the receiver is the identifier before
   the dot, whatever opened the reference. *)
let completion_after_handle_field_source_offers_its_methods () =
  let src = lib_src ^ "\npub struct other {\n  link: conn\n  y: ack = .link." in
  let line = List.length (String.split_on_char '\n' src) - 1 in
  let labels = completion_labels src (pos line 17) in
  Alcotest.(check (list string)) "the handle's methods" [ "publish" ] labels

let hover_on_a_handle_sourced_member_shows_the_call () =
  let src =
    lib_src
    ^ "\npub struct other {\n  link: conn\n  y: ack = .link.publish(\"t\")\n}\n"
  in
  let v = hover_value src (at src "y: ack") in
  Alcotest.(check bool)
    "the source is in the hover" true
    (contains v "y: ack = .link.publish(\"t\")")

let completion_after_unknown_namespace_is_empty () =
  let src = lib_src ^ "\npub struct other {\n  y: ack = nothing." in
  let line = List.length (String.split_on_char '\n' src) - 1 in
  Alcotest.(check (list string))
    "nothing to offer" []
    (completion_labels src (pos line 19))

let () =
  Alcotest.run "analysis_ext"
    [
      ( "vocabulary",
        [
          Alcotest.test_case "ext construct doc" `Quick
            ext_construct_doc_is_present;
          Alcotest.test_case "docs cover the grammar" `Quick
            ext_lib_docs_cover_the_grammar;
          Alcotest.test_case "parser accepts the vocabulary" `Quick
            parser_accepts_the_vocabulary;
        ] );
      ( "hover",
        [
          Alcotest.test_case "construct words" `Quick hover_construct_words;
          Alcotest.test_case "request reference" `Quick hover_request_reference;
          Alcotest.test_case "qualified request" `Quick
            hover_qualified_request_is_not_the_reference;
          Alcotest.test_case "named extern and handle" `Quick
            hover_named_extern_and_handle;
          Alcotest.test_case "field named like a word" `Quick
            hover_field_named_like_a_word_is_not_a_construct;
        ] );
      ( "outline",
        [
          Alcotest.test_case "the example" `Quick outline_of_the_example;
          Alcotest.test_case "a handle" `Quick outline_of_a_handle;
        ] );
      ( "completion",
        [
          Alcotest.test_case "language block" `Quick
            completion_in_a_language_block;
          Alcotest.test_case "ext body" `Quick completion_in_an_ext_body;
          Alcotest.test_case "type position inside" `Quick
            completion_type_position_inside_a_block;
          Alcotest.test_case "ext name call site" `Quick
            completion_after_ext_name_offers_its_externs;
          Alcotest.test_case "handle field call site" `Quick
            completion_after_handle_field_offers_its_methods;
          Alcotest.test_case "handle-sourced member offers its methods" `Quick
            completion_after_handle_field_source_offers_its_methods;
          Alcotest.test_case "hover on a handle-sourced member" `Quick
            hover_on_a_handle_sourced_member_shows_the_call;
          Alcotest.test_case "unknown namespace" `Quick
            completion_after_unknown_namespace_is_empty;
        ] );
    ]
