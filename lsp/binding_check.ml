(* Foreign-binding diagnostics in the editor: the pairs an [ext] declares,
   one per language, and how the report `tono check --json` prints maps onto
   LSP diagnostics. Everything here is pure; [Binding_runner] owns the
   processes and the threads.

   The unit of work is the pair (ext, language): one probe, one verdict.
   Each pair has a key over what its verdict depends on, so an edit inside
   one [go { }] block dirties that pair alone, while an edit to what every
   language crosses (an op's signature, a shared type) dirties them all. *)

open Lsp.Types
module Ast = Tono_frontend.Ast
module Span = Tono_frontend.Span
module Ext_sites = Tono_frontend.Ext_sites

type pair = { ext : string; lang : string }

(* The check accepts either spelling and reports under the short one. *)
let normalize_lang = function "typescript" -> "ts" | l -> l

(* --- the language regions of an ext --- *)

let ext_regions (name : string) (body : Ast.ext_lib_body) :
    (pair * Span.span) list =
  let pair lang = { ext = name; lang = normalize_lang lang } in
  let paths =
    List.map
      (fun (lp : Ast.lang_path) ->
        (pair lp.lp_lang, Span.merge lp.lp_lang_span lp.lp_path_span))
      body.elib_langs
  in
  let blocks (bs : Ast.lang_block list) =
    List.map (fun (b : Ast.lang_block) -> (pair b.lb_lang, b.lb_span)) bs
  in
  let bodies (d : Ast.extern_decl) =
    List.map
      (fun (b : Ast.extern_lang_body) -> (pair b.elb_lang, b.elb_span))
      d.ed_langs
  in
  paths
  @ List.concat_map
      (fun (s : Ast.foreign_struct) -> blocks s.fs_langs)
      body.elib_structs
  @ List.concat_map
      (fun (t : Ast.opaque_type) ->
        blocks t.opq_langs @ List.concat_map bodies t.opq_methods)
      body.elib_types
  @ List.concat_map bodies body.elib_externs

let ext_decls (f : Ast.file) : (string * Ast.decl * Ast.ext_lib_body) list =
  List.filter_map
    (fun (d : Ast.decl) ->
      match d.dkind with
      | Ast.DExtLib { body; _ } -> Some (d.dname, d, body)
      | _ -> None)
    f.decls

(* Every pair the file declares, in order of first appearance, with the
   regions of text that belong to that language alone. *)
let regions (f : Ast.file) : (pair * Span.span list) list =
  let all =
    List.concat_map (fun (name, _, body) -> ext_regions name body) (ext_decls f)
  in
  List.fold_left
    (fun acc (p, s) ->
      if List.mem_assoc p acc then
        List.map (fun (q, ss) -> if q = p then (q, ss @ [ s ]) else (q, ss)) acc
      else acc @ [ (p, [ s ]) ])
    [] all

let pairs (f : Ast.file) : pair list = List.map fst (regions f)

(* --- the pair key --- *)

let clamp n i = max 0 (min n i)

let slice ~(text : string) (s : Span.span) : string =
  let n = String.length text in
  let a = clamp n s.start.offset and b = clamp n s.finish.offset in
  if b > a then String.sub text a (b - a) else ""

(* [text] with every span cut out: what is left is what every language
   crosses. *)
let without ~(text : string) (spans : Span.span list) : string =
  let n = String.length text in
  let spans =
    List.sort
      (fun (a : Span.span) (b : Span.span) ->
        compare a.start.offset b.start.offset)
      spans
  in
  let buf = Buffer.create n in
  let cursor =
    List.fold_left
      (fun cur (s : Span.span) ->
        let a = max cur (clamp n s.start.offset) in
        let b = max cur (clamp n s.finish.offset) in
        Buffer.add_substring buf text cur (a - cur);
        b)
      0 spans
  in
  Buffer.add_substring buf text cursor (n - cursor);
  Buffer.contents buf

(* One TOML table of the manifest by its header line, up to the next table.
   A line scan is enough: the key only needs the text to change when the
   table does. *)
let section ~(manifest : string) (header : string) : string =
  let lines = String.split_on_char '\n' manifest in
  let rec skip = function
    | [] -> []
    | l :: rest -> if String.trim l = header then take rest else skip rest
  and take = function
    | [] -> []
    | l :: rest ->
        let t = String.trim l in
        if String.length t > 0 && t.[0] = '[' then [] else l :: take rest
  in
  String.concat "\n" (skip lines)

(* What the manifest pins for the pair: the ext's version for that language
   and the target's layout, which is where the check resolves the library. *)
let pin ~(manifest : string option) (p : pair) : string =
  match manifest with
  | None -> ""
  | Some m ->
      let target = match p.lang with "ts" -> "typescript" | l -> l in
      let keyed_to_lang line =
        match String.index_opt line '=' with
        | Some i ->
            let k = String.trim (String.sub line 0 i) in
            normalize_lang k = p.lang
        | None -> false
      in
      let versions =
        List.filter keyed_to_lang
          (String.split_on_char '\n'
             (section ~manifest:m ("[ext." ^ p.ext ^ "]")))
      in
      String.concat "\n" versions
      ^ "\n"
      ^ section ~manifest:m ("[target." ^ target ^ "]")

let key ~(text : string) ~(manifest : string option) (f : Ast.file) (p : pair) :
    string =
  let all = regions f in
  let every = List.concat_map snd all in
  let own = Option.value ~default:[] (List.assoc_opt p all) in
  Digest.to_hex
    (Digest.string
       (String.concat "\000"
          [
            without ~text every;
            String.concat "\001" (List.map (slice ~text) own);
            pin ~manifest p;
          ]))

(* --- the report as diagnostics --- *)

(* Where a note about the whole pair is shown: its path line, else the ext's
   name. *)
let anchor (f : Ast.file) (p : pair) : Span.span option =
  List.find_map
    (fun (name, (d : Ast.decl), (body : Ast.ext_lib_body)) ->
      if name <> p.ext then None
      else
        match
          List.find_opt
            (fun (lp : Ast.lang_path) -> normalize_lang lp.lp_lang = p.lang)
            body.elib_langs
        with
        | Some lp -> Some (Span.merge lp.lp_lang_span lp.lp_path_span)
        | None -> Some d.dname_span)
    (ext_decls f)

(* The byte offset of a 1-based line and byte column, clamped to the text. *)
let pos_of ~(text : string) (line : int) (col : int) : Span.pos =
  let n = String.length text in
  let rec start i l =
    if l <= 1 || i >= n then i
    else if text.[i] = '\n' then start (i + 1) (l - 1)
    else start (i + 1) l
  in
  let offset = clamp n (start 0 line + max 0 (col - 1)) in
  { Span.line; col; offset }

(* The check prints spans as [Span.to_string] does. *)
let span_of_string ~(text : string) (s : string) : Span.span option =
  let ints parts = List.map int_of_string_opt parts in
  match String.split_on_char '-' s with
  | [ a; b ] -> (
      match
        (ints (String.split_on_char ':' a), ints (String.split_on_char ':' b))
      with
      | [ Some l; Some c ], [ Some c' ] ->
          Some { Span.start = pos_of ~text l c; finish = pos_of ~text l c' }
      | [ Some l; Some c ], [ Some l'; Some c' ] ->
          Some { Span.start = pos_of ~text l c; finish = pos_of ~text l' c' }
      | _ -> None)
  | _ -> None

let member name (json : Yojson.Safe.t) =
  match json with `Assoc fields -> List.assoc_opt name fields | _ -> None

let string_member name json =
  match member name json with Some (`String s) -> Some s | _ -> None

(* The site a finding names, found again in the current parse: the verdict
   survives edits elsewhere in the file, and the range follows the binding. *)
let site_span (sites : Ext_sites.site list) (site : Yojson.Safe.t) :
    Span.span option =
  let want k = string_member k site in
  List.find_map
    (fun (s : Ext_sites.site) ->
      if
        Some s.ext = want "ext"
        && Option.map normalize_lang (want "lang")
           = Some (normalize_lang s.lang)
        && Some (Ext_sites.kind_to_string s.kind) = want "kind"
        && s.owner = want "owner"
        && s.name = want "name"
      then Some s.span
      else None)
    sites

let diagnostic ~text ~severity ~code ~data span message : Diagnostic.t =
  Diagnostic.create
    ~range:(Analysis.range_of_span ~text span)
    ~severity
    ?code:(Option.map (fun c -> `String c) code)
    ~source:"tono" ~message:(`String message) ~data ()

(* One report line as the diagnostic the editor shows. A finding is an
   error at its binding; what was left unchecked is a note at the pair's
   anchor, and so is a check that could not run at all, one level louder;
   what passed shows nothing. A line that is not one of those is shown as
   is, never dropped: an unreadable report must not read as a clean one. *)
let diagnostic_of_line ~(text : string) ~(sites : Ext_sites.site list)
    ~(anchor : Span.span) (raw : string) : Diagnostic.t option =
  let json = try Some (Yojson.Safe.from_string raw) with _ -> None in
  let message = Option.bind json (string_member "message") in
  match (Option.bind json (string_member "kind"), message, json) with
  | Some "finding", Some message, Some json ->
      let span =
        match Option.bind (member "site" json) (site_span sites) with
        | Some s -> s
        | None -> (
            match
              Option.bind (string_member "span" json) (span_of_string ~text)
            with
            | Some s -> s
            | None -> anchor)
      in
      Some
        (diagnostic ~text ~severity:DiagnosticSeverity.Error
           ~code:(string_member "code" json)
           ~data:json span message)
  | Some "unchecked", Some message, Some json ->
      Some
        (diagnostic ~text ~severity:DiagnosticSeverity.Information ~code:None
           ~data:json anchor
           ("not checked: " ^ message))
  | Some "error", Some message, Some json ->
      Some
        (diagnostic ~text ~severity:DiagnosticSeverity.Warning ~code:None
           ~data:json anchor
           ("not checked: " ^ message))
  | Some "checked", Some _, _ -> None
  | _ ->
      Some
        (diagnostic ~text ~severity:DiagnosticSeverity.Warning ~code:None
           ~data:(`String raw) anchor
           ("binding check: unreadable report line: " ^ raw))

let whole_file_start : Span.span =
  let p = { Span.line = 1; col = 1; offset = 0 } in
  { Span.start = p; finish = p }

let diagnostics_of_lines ~(text : string) ~(file : Ast.file) (p : pair)
    (lines : string list) : Diagnostic.t list =
  let sites = Ext_sites.of_file file in
  let anchor = Option.value ~default:whole_file_start (anchor file p) in
  List.filter_map
    (fun raw ->
      if String.trim raw = "" then None
      else diagnostic_of_line ~text ~sites ~anchor raw)
    lines

(* The report line a diagnostic was made from, printed the way `tono check`
   prints it: what proves the editor and the command agree. *)
let report_line (d : Diagnostic.t) : string option =
  match d.data with
  | Some json -> (
      match (string_member "kind" json, string_member "message" json) with
      | Some "finding", Some message ->
          Some
            (Printf.sprintf "%s: error: %s: %s"
               (Option.value ~default:"?" (string_member "span" json))
               (Option.value ~default:"?" (string_member "code" json))
               message)
      | Some "unchecked", Some message -> Some ("not checked: " ^ message)
      | Some "error", Some message -> Some message
      | _ -> None)
  | None -> None
