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

(* --- where the cursor is, read off the token stream --- *)

(* The braces of an ext block nest as ext { extern { lang { ... } } } and
   ext { type { extern { lang { ... } } } }; the frame kind is what decides
   which words read as constructs and which completions apply. Read from
   tokens rather than the AST because the parser collapses the span of an
   unclosed block, and completion runs mid-edit, on unclosed blocks. *)
type frame = Ext | Extern | Lang | Type | Other

(* The word that will name the next frame opened at the current depth: only a
   block word standing where a declaration begins (after `{`, `}`, `,`, or
   the string closing a language path) counts, so an extern parameter named
   `type` does not. *)
let starts_decl (prev : Token.kind option) : bool =
  match prev with
  | None | Some (Token.LBrace | Token.RBrace | Token.Comma | Token.Str _) ->
      true
  | Some _ -> false

type state = {
  stack : frame list;
  pending : string option; (* the block word seen since the last brace *)
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
        | _, _, Some (Token.Ident _), Some Token.KwExt -> Ext
        | (Ext | Type), Some "extern", _, _ -> Extern
        | Ext, Some "type", _, _ -> Type
        | Extern, _, Some (Token.Ident _), _ -> Lang
        | _ -> Other
      in
      { st' with stack = frame :: st.stack; pending = None }
  | Token.RBrace -> (
      match st.stack with
      | _ :: rest -> { st' with stack = rest; pending = None }
      | [] -> st')
  | Token.Ident w
    when List.mem w Vocab.block_words
         && (match top st with Ext | Type -> true | _ -> false)
         && starts_decl st.prev ->
      { st' with pending = Some w }
  | _ -> st'

let initial = { stack = []; pending = None; prev = None; prev2 = None }

(* The frame the cursor is in: fold the tokens that end before [off]. *)
let frame_at (toks : Token.t list) (off : int) : frame =
  let st =
    List.fold_left
      (fun st (t : Token.t) ->
        if t.span.finish.offset <= off then step st t.kind else st)
      initial toks
  in
  top st

(* The construct word the token at [off] spells, if any: extern/type where a
   declaration begins in an ext (or type) body, a language-block field or
   marker inside a language block, and the leading-dot `.request` reference
   anywhere (a dot after an identifier is a path or a qualifier, so
   `http.request` is not it). *)
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
            when List.mem w Vocab.block_words
                 && (match top st with Ext | Type -> true | _ -> false)
                 && starts_decl st.prev ->
              Some w
          | Token.Ident w when List.mem w Vocab.lang_body_words && top st = Lang
            ->
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
        ~detail:("extern " ^ extern_signature e)
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
    let name =
      match t.Ast.opq_instance with
      | None -> t.Ast.opq_name
      | Some i ->
          Printf.sprintf "%s (%s[%s])" t.Ast.opq_name i.Ast.oi_foreign_name
            (Printer.print_ty i.Ast.oi_arg)
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
