(* Browser entry for the OCaml frontend: the same parse, typecheck, and IR
   pipeline that backs the CLI, exported to JavaScript so the playground reuses
   the frontend as the single source of truth instead of re-implementing any of
   it. Spans cross the boundary as byte offsets; the web app maps them to
   JavaScript string indices because js_of_ocaml strings are UTF-8 encoded. *)
open Js_of_ocaml
module F = Tono_frontend

let js_opt_string = function
  | None -> Js.Opt.empty
  | Some s -> Js.Opt.return (Js.string s)

let diag_to_js (d : F.Diagnostic.t) =
  let open F.Span in
  object%js
    val message = Js.string d.F.Diagnostic.message
    val code = js_opt_string d.F.Diagnostic.code

    val severity =
      Js.string
        (match d.F.Diagnostic.severity with
        | F.Diagnostic.Error -> "error"
        | F.Diagnostic.Warning -> "warning")

    val line = d.F.Diagnostic.span.start.line
    val col = d.F.Diagnostic.span.start.col
    val startOffset = d.F.Diagnostic.span.start.offset
    val endOffset = d.F.Diagnostic.span.finish.offset
  end

(* Mirror of [compile_to_json], but returning the diagnostics structured
   instead of joined into one string, so the editor can place them inline. *)
let compile_js src_js =
  let src = Js.to_string src_js in
  let m, diags = F.compile ~module_name:"playground" src in
  let has_error =
    List.exists
      (fun (d : F.Diagnostic.t) -> d.F.Diagnostic.severity = F.Diagnostic.Error)
      diags
  in
  let ir =
    if has_error then Js.Opt.empty
    else
      let m = F.Protocol_http.resolve_module m in
      let model =
        { F.Ir.tono_ir_version = F.Ir_json.current_ir_version; modules = [ m ] }
      in
      Js.Opt.return
        (Js.string (F.Ir_json.to_canonical_string (F.Ir_json.encode_model model)))
  in
  object%js
    val ir = ir
    val diagnostics = Js.array (Array.of_list (List.map diag_to_js diags))
  end

let format_js src_js =
  match F.format_source (Js.to_string src_js) with
  | Ok formatted ->
      object%js
        val formatted = Js.Opt.return (Js.string formatted)
        val error = Js.Opt.empty
      end
  | Error msg ->
      object%js
        val formatted = Js.Opt.empty
        val error = Js.Opt.return (Js.string msg)
      end

(* Coarse highlighting families derived from the real lexer, so the editor
   never needs a second grammar that could drift from the parser. *)
let token_family (k : F.Token.kind) =
  match k with
  | F.Token.KwStruct | F.Token.KwEnum | F.Token.KwUnion | F.Token.KwOp
  | F.Token.KwMap | F.Token.KwPub | F.Token.KwImport | F.Token.KwAs
  | F.Token.KwExt ->
      "keyword"
  | F.Token.Prim _ -> "type"
  | F.Token.Str _ -> "string"
  | F.Token.Int _ | F.Token.Float _ -> "number"
  | F.Token.Ident _ -> "ident"
  | F.Token.At -> "attribute"
  | F.Token.Eof -> "eof"
  | _ -> "punct"

let tokens_js src_js =
  let tokens, _diags = F.Lexer.tokenize (Js.to_string src_js) in
  let visible =
    List.filter (fun (t : F.Token.t) -> t.F.Token.kind <> F.Token.Eof) tokens
  in
  Js.array
    (Array.of_list
       (List.map
          (fun (t : F.Token.t) ->
            object%js
              val family = Js.string (token_family t.F.Token.kind)
              val startOffset = t.F.Token.span.F.Span.start.F.Span.offset
              val endOffset = t.F.Token.span.F.Span.finish.F.Span.offset
            end)
          visible))

(* ── IDE features, backed by the LSP's pure analysis core ─────────────────
   [Tono_lsp_lib.Analysis] is the exact logic behind the editor LSP server;
   only the transport differs (direct calls instead of JSON-RPC). Positions
   cross the boundary as LSP positions: 0-based line, UTF-16 character. *)
module A = Tono_lsp_lib.Analysis
open Lsp.Types

let position ~line ~character = Position.create ~line ~character

let pos_to_js (p : Position.t) =
  object%js
    val line = p.line
    val character = p.character
  end

let range_to_js (r : Range.t) =
  object%js
    val start = pos_to_js r.start
    val end_ = pos_to_js r.end_
  end

let completions_js src_js line character =
  let text = Js.to_string src_js in
  let file = A.parse text in
  let items = A.completions ~text ~file (position ~line ~character) in
  Js.array
    (Array.of_list
       (List.map
          (fun (ci : CompletionItem.t) ->
            object%js
              val label = Js.string ci.label
              val detail = js_opt_string ci.detail
              val insertText = js_opt_string ci.insertText
            end)
          items))

let hover_js src_js line character =
  let text = Js.to_string src_js in
  let file = A.parse text in
  match A.hover_at ~markdown:false ~text ~file (position ~line ~character) with
  | None -> Js.Opt.empty
  | Some (h : Hover.t) ->
      let value =
        match h.contents with
        | `MarkupContent (m : MarkupContent.t) -> m.value
        | `MarkedString (m : MarkedString.t) -> (
            match m with
            | { value; language = _ } -> value)
        | `List ms ->
            String.concat "\n\n"
              (List.map (fun (m : MarkedString.t) -> m.value) ms)
      in
      Js.Opt.return
        (object%js
           val contents = Js.string value
           val range = Js.Opt.option (Option.map range_to_js h.range)
        end)

let definition_js src_js line character =
  let text = Js.to_string src_js in
  let file = A.parse text in
  let uri = DocumentUri.of_path "playground.tono" in
  match A.definition_at ~uri ~text ~file (position ~line ~character) with
  | None -> Js.Opt.empty
  | Some (loc : Location.t) -> Js.Opt.return (range_to_js loc.range)

(* The declaration outline the cross-target highlight resolves against: every
   declaration (entry ops flattened as [entry.op], matching their canonical IR
   ids) with its name span. The surface AST has no body span, so consumers
   treat [start of name .. start of next name] as the declaration's extent,
   the same heuristic the LSP uses. *)
let decl_kind_word (d : F.Ast.decl) =
  match d.F.Ast.dkind with
  | F.Ast.DStruct _ -> "struct"
  | F.Ast.DEnum _ -> "enum"
  | F.Ast.DUnion _ -> "union"
  | F.Ast.DOp _ -> "op"
  | F.Ast.DExt _ -> "ext"

let decls_js src_js =
  let text = Js.to_string src_js in
  let file = A.parse text in
  let flat =
    List.concat_map
      (fun (d : F.Ast.decl) ->
        let nested =
          match d.F.Ast.dkind with
          | F.Ast.DStruct { ops; _ } ->
              List.map
                (fun (o : F.Ast.decl) ->
                  (d.F.Ast.dname ^ "." ^ o.F.Ast.dname, o))
                ops
          | _ -> []
        in
        (d.F.Ast.dname, d) :: nested)
      file.F.Ast.decls
  in
  Js.array
    (Array.of_list
       (List.map
          (fun (dotted, (d : F.Ast.decl)) ->
            object%js
              val name = Js.string dotted
              val kind = Js.string (decl_kind_word d)
              val nameStart = d.F.Ast.dname_span.F.Span.start.F.Span.offset
              val nameEnd = d.F.Ast.dname_span.F.Span.finish.F.Span.offset
            end)
          flat))

let () =
  Js.export "tonoFrontend"
    (object%js
       method compile (s : Js.js_string Js.t) = compile_js s
       method formatSource (s : Js.js_string Js.t) = format_js s
       method tokens (s : Js.js_string Js.t) = tokens_js s
       method decls (s : Js.js_string Js.t) = decls_js s

       method completionsAt (s : Js.js_string Js.t) (line : int)
           (character : int) =
         completions_js s line character

       method hoverAt (s : Js.js_string Js.t) (line : int) (character : int) =
         hover_js s line character

       method definitionAt (s : Js.js_string Js.t) (line : int)
           (character : int) =
         definition_js s line character

       method irVersion = F.Ir_json.current_ir_version
       method version = Js.string F.version
    end)
