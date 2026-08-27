(* The editor's view of the "ext" library block: which contextual word sits
   under the cursor, which block the cursor is in, the outline of a block,
   and the externs a call site can name. Split from [Analysis] so that file
   stays under its size ceiling; [Analysis] owns positions and hover shapes
   and calls in here. Every word set comes from [Ext_lib_vocab] (the
   grammar's own list), never from a second copy. *)

open Lsp.Types
module Ast = Tono_frontend.Ast
module Span = Tono_frontend.Span
module Token = Tono_frontend.Token
module Printer = Tono_frontend.Printer
module Vocab = Tono_frontend.Ext_lib_vocab

(* End-inclusive: editors place the caret at a token's end constantly, and
   the convention (rust-analyzer, gopls) is that the position equal to the
   range end still resolves the token to its left. The grammar separates
   names with punctuation, so end-of-one is never start-of-another name. *)
let contains (s : Span.span) (off : int) : bool =
  s.start.offset <= off && off <= s.finish.offset

(* [typescript] and [ts] are one language to every consumer of a block. *)
let normalize_lang = function "typescript" -> "ts" | l -> l

(* --- where the cursor is, read off the token stream --- *)

(* The braces of an ext block nest as ext { op { lang { ... } } } and
   ext { struct { op { lang { ... } } } }, with a language block of the
   header or of a struct (lang { #(...) }) opening directly under Ext or
   Struct; the frame kind is what decides which words read as constructs and
   which completions apply. Read from tokens rather than the AST because the
   parser collapses the span of an unclosed block, and completion runs
   mid-edit, on unclosed blocks. *)
type frame =
  | Ext of string (* the ext's name *)
  | Struct
  | Op
  | Lang of string (* the language word that opened the block *)
  | Block of string
  | Other

type state = {
  stack : frame list;
  pending : Token.kind option;
      (* the declaration keyword since the last brace *)
  prev : Token.kind option;
  prev2 : Token.kind option;
}

let top (st : state) : frame = match st.stack with f :: _ -> f | [] -> Other

let step (st : state) (k : Token.kind) : state =
  let st' = { st with prev = Some k; prev2 = st.prev } in
  match k with
  | Token.LBrace ->
      let frame =
        match (top st, st.pending, st.prev, st.prev2) with
        | _, _, Some (Token.Ident name), Some Token.KwExt -> Ext name
        | Ext _, Some Token.KwStruct, _, _ -> Struct
        | (Ext _ | Struct), Some Token.KwOp, _, _ -> Op
        | Op, _, Some (Token.Ident lang), _ -> Lang lang
        | (Ext _ | Struct), None, Some (Token.Ident lang), _ -> Block lang
        | _ -> Other
      in
      { st' with stack = frame :: st.stack; pending = None }
  | Token.RBrace -> (
      match st.stack with
      | _ :: rest -> { st' with stack = rest; pending = None }
      | [] -> st')
  | (Token.KwStruct | Token.KwOp) as kw
    when match top st with Ext _ | Struct -> true | _ -> false ->
      { st' with pending = Some kw }
  | _ -> st'

let initial = { stack = []; pending = None; prev = None; prev2 = None }

(* The state after the tokens that end before [off]: the frame stack the
   cursor is in and the two tokens before it. *)
let state_at (toks : Token.t list) (off : int) : state =
  List.fold_left
    (fun st (t : Token.t) ->
      if t.span.finish.offset <= off then step st t.kind else st)
    initial toks

(* The frame the cursor is in. *)
let frame_at (toks : Token.t list) (off : int) : frame = top (state_at toks off)

(* The ext that owns a frame stack, if one does. *)
let ext_of (stack : frame list) : string option =
  List.find_map (function Ext name -> Some name | _ -> None) stack

(* The construct word the token at [off] spells, if any: a language-block
   line (call/yields/returns) inside a language block, and the leading-dot
   `.request` reference anywhere (a dot after an identifier is a path or a
   qualifier, so `http.request` is not it). The block's declarations are
   keywords, which [Analysis] already resolves on its own. *)
let word_at (toks : Token.t list) (off : int) : string option =
  let is_ident (t : Token.t) =
    match t.kind with Token.Ident _ -> true | _ -> false
  in
  let rec go st = function
    | [] -> None
    (* Only an identifier can be a construct word, so a punctuation token
       whose end touches [off] (the dot of `.request`) is stepped over. *)
    | (t : Token.t) :: rest ->
        if contains t.span off && is_ident t then
          match t.kind with
          | Token.Ident w
            when List.mem w Vocab.lang_fields
                 && match top st with Lang _ -> true | _ -> false ->
              Some w
          | Token.Ident w
            when String.equal w Vocab.request_ref
                 && st.prev = Some Token.Dot
                 && not
                      (match st.prev2 with
                      | Some (Token.Ident _) -> true
                      | _ -> false) ->
              Some w
          | _ -> None
        else go (step st t.kind) rest
  in
  go initial toks

(* --- what a block offers --- *)

(* A construct word as a completion item, documented by the same prose the
   hover shows, so the two never drift. *)
let ext_word_item ?(suffix = "") (word : string) : CompletionItem.t =
  let documentation =
    Option.map (fun d -> `String d) (Hover_docs.construct_doc word)
  in
  CompletionItem.create ~label:word ~kind:CompletionItemKind.Keyword
    ~detail:"ext block" ~insertText:(word ^ suffix) ?documentation ()

(* The words a block accepts: the declarations of an ext body or a handle
   body, and the lines (as `word:`) of a language block. *)
let ext_frame_items (frame : frame) : CompletionItem.t list option =
  match frame with
  | Ext _ -> Some (List.map ext_word_item [ "struct"; "op" ])
  | Struct -> Some [ ext_word_item "op" ]
  | Lang _ -> Some (List.map (ext_word_item ~suffix:": ") Vocab.lang_fields)
  | Op | Block _ | Other -> None

(* --- the declared externs --- *)

let ext_libs (file : Ast.file) : (Ast.decl * Ast.ext_lib_body) list =
  List.filter_map
    (fun (d : Ast.decl) ->
      match d.Ast.dkind with
      | Ast.DExtLib { body; _ } -> Some (d, body)
      | _ -> None)
    file.Ast.decls

let all_opaque_types (file : Ast.file) : Ast.opaque_type list =
  List.concat_map (fun (_, b) -> b.Ast.elib_types) (ext_libs file)

let extern_signature (e : Ast.extern_decl) : string =
  "("
  ^ String.concat ", "
      (List.map
         (fun (p : Ast.extern_param) ->
           p.Ast.ep_name ^ ": " ^ Printer.print_ty p.ep_type)
         e.Ast.ed_params)
  ^ "): "
  ^ Printer.print_ty e.Ast.ed_return

let extern_items (externs : Ast.extern_decl list) : CompletionItem.t list =
  List.map
    (fun (e : Ast.extern_decl) ->
      CompletionItem.create ~label:e.Ast.ed_name ~kind:CompletionItemKind.Method
        ~detail:("op " ^ extern_signature e)
        ())
    externs

(* What `ns.` offers at a call site: the free externs of the ext named [ns],
   or the methods of the opaque handle a field named [ns] holds. *)
let call_items ~(fields : Ast.member list) (file : Ast.file) (ns : string) :
    CompletionItem.t list =
  match
    List.find_opt (fun ((d : Ast.decl), _) -> d.Ast.dname = ns) (ext_libs file)
  with
  | Some (_, body) -> extern_items body.Ast.elib_externs
  | None -> (
      let handle =
        List.find_map
          (fun (m : Ast.member) ->
            if m.Ast.mname <> ns then None
            else
              match m.Ast.mtype with
              | Ast.TName (n, _, _) | Ast.TQName (_, n, _, _) -> Some n
              | _ -> None)
          fields
      in
      match handle with
      | None -> []
      | Some n -> (
          match
            List.find_opt
              (fun (t : Ast.opaque_type) -> t.Ast.opq_name = n)
              (all_opaque_types file)
          with
          | Some t -> extern_items t.Ast.opq_methods
          | None -> []))

(* --- hover on a declared name inside a block --- *)

(* The extern or opaque handle whose name sits at [off], rendered by the fmt
   printer as it is written in the block, for the same hover shape a
   declaration gets. *)
let named_at (file : Ast.file) (off : int) : (string * Span.span) option =
  List.find_map
    (fun (_, (b : Ast.ext_lib_body)) ->
      let extern_hit (e : Ast.extern_decl) =
        if contains e.Ast.ed_name_span off then
          Some (String.trim (Printer.print_extern ~indent:"" e), e.ed_name_span)
        else None
      in
      let type_hit (t : Ast.opaque_type) =
        if contains t.Ast.opq_name_span off then
          Some
            ( String.trim (Printer.print_opaque_type ~indent:"" t),
              t.opq_name_span )
        else List.find_map extern_hit t.Ast.opq_methods
      in
      match List.find_map extern_hit b.Ast.elib_externs with
      | Some hit -> Some hit
      | None -> List.find_map type_hit b.Ast.elib_types)
    (ext_libs file)

(* --- outline --- *)

(* The children of an ext block, in source order: foreign structs with their
   fields, opaque handles with their methods, and free externs. *)
let symbols ~(range : Span.span -> Range.t) (b : Ast.ext_lib_body) :
    DocumentSymbol.t list =
  let node ~kind ~name ~span ~children =
    DocumentSymbol.create ~kind ~name ~range:(range span)
      ~selectionRange:(range span) ~children ()
  in
  let extern_sym (e : Ast.extern_decl) =
    ( e.Ast.ed_span.Span.start.offset,
      node ~kind:SymbolKind.Method
        ~name:(e.Ast.ed_name ^ extern_signature e)
        ~span:e.Ast.ed_name_span ~children:[] )
  in
  let struct_sym (s : Ast.foreign_struct) =
    ( s.Ast.fs_span.Span.start.offset,
      node ~kind:SymbolKind.Struct ~name:s.Ast.fs_name ~span:s.Ast.fs_name_span
        ~children:
          (List.map
             (fun (f : Ast.foreign_field) ->
               node ~kind:SymbolKind.Field ~name:f.Ast.ff_name
                 ~span:f.Ast.ff_name_span ~children:[])
             s.Ast.fs_fields) )
  in
  let type_sym (t : Ast.opaque_type) =
    (* The outline shows how each target holds the handle, the one thing
       its header says. *)
    let name =
      match t.Ast.opq_langs with
      | [] -> t.Ast.opq_name
      | langs ->
          Printf.sprintf "%s (%s)" t.Ast.opq_name
            (String.concat ", "
               (List.map
                  (fun (b : Ast.lang_block) -> b.Ast.lb_lang ^ ": " ^ b.lb_head)
                  langs))
    in
    ( t.Ast.opq_span.Span.start.offset,
      node ~kind:SymbolKind.Class ~name ~span:t.Ast.opq_name_span
        ~children:(List.map (fun e -> snd (extern_sym e)) t.Ast.opq_methods) )
  in
  List.map struct_sym b.Ast.elib_structs
  @ List.map type_sym b.Ast.elib_types
  @ List.map extern_sym b.Ast.elib_externs
  |> List.sort (fun (a, _) (b, _) -> compare a b)
  |> List.map snd
