(* Completion inside [#(...)], read off the token stream. The spelling is
   one [Foreign] token the lexer never looks into (its bytes are the
   target's), so the position is decided by what precedes the token: the
   [call:] word, a parameter's colon, the brace that opens a language
   block. The frames of [Analysis_ext] say which block that is and which
   ext owns it; the language is the block's own word. The prefix inside the
   spelling then splits on the last [.] or [::] for a member access.

   The token stream is read rather than the AST because completion runs
   mid-edit, on a spelling that is not closed yet: the lexer still emits
   the token (to the end of the input) and every token before it. *)

open Lsp.Types
module Ast = Tono_frontend.Ast
module Span = Tono_frontend.Span
module Token = Tono_frontend.Token
module Vocab = Tono_frontend.Ext_lib_vocab
module FI = Foreign_index

type site = {
  ext : string;
  lang : string;
  position : FI.position;
  prefix : string;
}

(* The [Foreign] token the cursor is inside: after the [#(] and before the
   closing paren, or up to the end of an unterminated one (its span then
   runs to the end of the input, with no paren to sit after). *)
let foreign_at (toks : Token.t list) (off : int) : (Token.t * string) option =
  List.find_map
    (fun (t : Token.t) ->
      match t.kind with
      | Token.Foreign body ->
          let start = t.span.start.offset and finish = t.span.finish.offset in
          let terminated = finish - start = String.length body + 3 in
          if
            start + 2 <= off
            && (off < finish || ((not terminated) && off = finish))
          then Some (t, body)
          else None
      | _ -> None)
    toks

let inside ~(toks : Token.t list) (off : int) : bool =
  Option.is_some (foreign_at toks off)

(* The last [::] or [.] of [s], whichever comes later. *)
let last_separator (s : string) : (int * int) option =
  let dot = String.rindex_opt s '.' in
  let rec colons i =
    if i < 1 then None
    else if s.[i] = ':' && s.[i - 1] = ':' then Some (i - 1)
    else colons (i - 1)
  in
  match (dot, colons (String.length s - 1)) with
  | None, None -> None
  | Some d, None -> Some (d, 1)
  | None, Some c -> Some (c, 2)
  | Some d, Some c -> if d > c then Some (d, 1) else Some (c, 2)

let strip_new (s : string) : string * bool =
  let n = String.length s in
  if n >= 4 && String.sub s 0 4 = "new " then
    (String.trim (String.sub s 4 (n - 4)), true)
  else (s, false)

(* The position of a spelling opened right after the tokens [st] folds:
   what the grammar puts a [#(...)] after. *)
let position_of (st : Analysis_ext.state) : FI.position option =
  let open Analysis_ext in
  let parent = match st.stack with _ :: p :: _ -> Some p | _ -> None in
  match (top st, st.prev, st.prev2) with
  | Block _, Some Token.LBrace, _ -> (
      match parent with
      | Some (Ext _) -> Some FI.Path
      | Some Struct -> Some FI.Type_pos
      | _ -> None)
  | Block _, Some Token.Colon, Some (Token.Ident _) -> Some FI.Type_pos
  | Lang _, Some Token.Colon, Some (Token.Ident "call") ->
      Some (FI.Call_head { after_new = false })
  | Lang _, Some Token.Colon, Some (Token.Ident _ | Token.RBrace) ->
      Some FI.Type_pos
  | Lang _, Some (Token.Dot | Token.LParen | Token.Comma), _ ->
      Some FI.Function_pos
  | _ -> None

let site_at ~(text : string) ~(toks : Token.t list) (off : int) : site option =
  match foreign_at toks off with
  | None -> None
  | Some (t, _) -> (
      let start = t.span.start.offset in
      let st = Analysis_ext.state_at toks start in
      match (Analysis_ext.ext_of st.stack, Analysis_ext.top st) with
      | Some ext, (Analysis_ext.Lang lang | Analysis_ext.Block lang) -> (
          match position_of st with
          | None -> None
          | Some base ->
              let prefix = String.sub text (start + 2) (off - start - 2) in
              let stripped, after_new = strip_new prefix in
              let base =
                match base with
                | FI.Call_head _ -> FI.Call_head { after_new }
                | other -> other
              in
              let position =
                match (base, last_separator stripped) with
                (* A module path has dots of its own and nothing to offer. *)
                | FI.Path, _ -> FI.Path
                | _, Some (i, _) ->
                    FI.Member { head = String.sub stripped 0 i; base }
                | _, None -> base
              in
              Some
                {
                  ext;
                  lang = Analysis_ext.normalize_lang lang;
                  position;
                  prefix;
                })
      | _ -> None)

let package_of (file : Ast.file) ~(ext : string) ~(lang : string) :
    string option =
  List.find_map
    (fun ((d : Ast.decl), (body : Ast.ext_lib_body)) ->
      if d.Ast.dname <> ext then None
      else
        List.find_map
          (fun (lp : Ast.lang_path) ->
            if Analysis_ext.normalize_lang lp.lp_lang = lang then
              Some lp.lp_path
            else None)
          body.elib_langs)
    (Analysis_ext.ext_libs file)

(* The word a Go author qualifies the library's names with: the last
   segment of the import path, minus a major-version suffix. *)
let go_selector (package : string) : string =
  let last =
    match String.rindex_opt package '/' with
    | Some i -> String.sub package (i + 1) (String.length package - i - 1)
    | None -> package
  in
  match String.rindex_opt last '.' with
  | Some i
    when i + 1 < String.length last
         && last.[i + 1] = 'v'
         && String.for_all
              (fun c -> c >= '0' && c <= '9')
              (String.sub last (i + 2) (String.length last - i - 2)) ->
      String.sub last 0 i
  | _ -> last

let completion ~(lookup : FI.lookup) ~(file : Ast.file) (site : site) :
    CompletionItem.t list =
  match lookup ~ext:site.ext ~lang:site.lang with
  | FI.Missing _ -> []
  | FI.Ready index ->
      let position =
        match site.position with
        (* [pkg.Name] in Go names the package the spelling is already
           qualified by: the top level again. *)
        | FI.Member { head; base }
          when site.lang = "go"
               && Option.map go_selector
                    (package_of file ~ext:site.ext ~lang:"go")
                  = Some head ->
            base
        | p -> p
      in
      FI.items index position

(* The language word under the cursor when it opens a block of an ext: an
   [Ident] naming a target, followed by [{], in a frame an ext owns. *)
let lang_hover ~(lookup : FI.lookup) ~(toks : Token.t list) (off : int) :
    (string * string option * Span.span) option =
  let rec go = function
    | (t : Token.t) :: ((next : Token.t) :: _ as rest) -> (
        match (t.kind, next.kind) with
        | Token.Ident word, Token.LBrace
          when List.mem word Vocab.targets && Analysis_ext.contains t.span off
          -> (
            let st = Analysis_ext.state_at toks t.span.start.offset in
            match (Analysis_ext.ext_of st.stack, Analysis_ext.top st) with
            | ( Some ext,
                (Analysis_ext.Ext _ | Analysis_ext.Struct | Analysis_ext.Op) )
              ->
                let lang = Analysis_ext.normalize_lang word in
                Some (word, Some (FI.describe (lookup ~ext ~lang)), t.span)
            | _ -> None)
        | _ -> go rest)
    | _ -> None
  in
  go toks
