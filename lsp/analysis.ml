(* The editor-facing analysis, kept free of any IO or JSON-RPC transport so the
   whole surface is unit-testable. Every answer is derived from the OCaml
   frontend (the single source of parsing and typechecking): diagnostics come
   from [Tono_frontend.compile], and navigation walks the surface [Ast] the
   parser produces. The server module is a thin stdio shell over this. *)

open Lsp.Types
module Ast = Tono_frontend.Ast
module Span = Tono_frontend.Span
module FDiag = Tono_frontend.Diagnostic

(* The frontend's [Span.pos] is 1-based line/column with a byte offset; an LSP
   [Position] is 0-based line/character. Columns are byte columns (the lexer
   scans bytes), so non-ASCII source drifts from the UTF-16 character counting a
   client may expect; acceptable for the current ASCII-oriented grammar. *)
let position_of_pos (p : Span.pos) : Position.t =
  Position.create ~line:(p.line - 1) ~character:(p.col - 1)

let range_of_span (s : Span.span) : Range.t =
  Range.create ~start:(position_of_pos s.start) ~end_:(position_of_pos s.finish)

(* Byte offset of an LSP position inside [text]: walk to the start of the target
   line, then add the character column, clamped to the document length. *)
let offset_of_position (text : string) (pos : Position.t) : int =
  let n = String.length text in
  let rec line_start i line =
    if line >= pos.line || i >= n then i
    else if text.[i] = '\n' then line_start (i + 1) (line + 1)
    else line_start (i + 1) line
  in
  min n (line_start 0 0 + pos.character)

let parse (src : string) : Ast.file = fst (Tono_frontend.Parser.parse src)

let lsp_diagnostics (src : string) : Diagnostic.t list =
  let _m, diags = Tono_frontend.compile src in
  List.map
    (fun (d : FDiag.t) ->
      let severity =
        match d.severity with
        | FDiag.Error -> DiagnosticSeverity.Error
        | FDiag.Warning -> DiagnosticSeverity.Warning
      in
      let code = Option.map (fun c -> `String c) d.code in
      Diagnostic.create ~range:(range_of_span d.span) ~severity ?code
        ~source:"tono" ~message:(`String d.message) ())
    diags

(* --- surface AST navigation --- *)

let contains (s : Span.span) (off : int) : bool =
  s.start.offset <= off && off < s.finish.offset

let decl_word (d : Ast.decl) : string =
  match d.dkind with
  | Ast.DStruct _ -> "struct"
  | Ast.DEnum _ -> "enum"
  | Ast.DUnion _ -> "union"
  | Ast.DOp _ -> "operation"
  | Ast.DExt { ekind = Ast.EHook; _ } -> "hook"
  | Ast.DExt { ekind = Ast.EContract; _ } -> "contract"
  | Ast.DExt { ekind = Ast.EConstraint; _ } -> "constraint"

(* Every named type reference inside a type, paired with its span. Primitives,
   type parameters, and error nodes carry no target so they are skipped. *)
let rec ty_refs (t : Ast.ty) (acc : (string * Span.span) list) :
    (string * Span.span) list =
  match t with
  | Ast.TName (name, args, span) ->
      List.fold_left (fun a t -> ty_refs t a) ((name, span) :: acc) args
  (* A module-qualified reference: the target lives in another file, so a local
     declaration lookup will miss, but hover still names it. *)
  | Ast.TQName (qual, name, args, span) ->
      List.fold_left
        (fun a t -> ty_refs t a)
        ((qual ^ "." ^ name, span) :: acc)
        args
  | Ast.TList (t, _) | Ast.TNullable (t, _) -> ty_refs t acc
  | Ast.TMap (k, v, _) -> ty_refs v (ty_refs k acc)
  | Ast.TPrim _ | Ast.TError _ -> acc

(* The types written inside a declaration (member types, union payloads,
   operation input/output). Enum cases carry no types. *)
let decl_tys (d : Ast.decl) : Ast.ty list =
  match d.dkind with
  | Ast.DStruct { members; _ } -> List.map (fun m -> m.Ast.mtype) members
  | Ast.DUnion { variants; _ } ->
      List.filter_map (fun v -> v.Ast.vpayload) variants
  | Ast.DOp { input; output } -> List.filter_map Fun.id [ input; output ]
  | Ast.DExt { esig; _ } -> (
      match esig with Some s -> [ s.Ast.esig_in; s.esig_out ] | None -> [])
  | Ast.DEnum _ -> []

let file_ty_refs (file : Ast.file) : (string * Span.span) list =
  List.concat_map
    (fun d -> List.concat_map (fun t -> ty_refs t []) (decl_tys d))
    file.Ast.decls

let find_decl (file : Ast.file) (name : string) : Ast.decl option =
  List.find_opt (fun (d : Ast.decl) -> d.dname = name) file.Ast.decls

let mk_hover (value : string) (span : Span.span) : Hover.t =
  let mc = MarkupContent.create ~kind:MarkupKind.PlainText ~value in
  Hover.create ~contents:(`MarkupContent mc) ~range:(range_of_span span) ()

let hover_at ~(text : string) ~(file : Ast.file) (pos : Position.t) :
    Hover.t option =
  let off = offset_of_position text pos in
  match
    List.find_opt
      (fun (d : Ast.decl) -> contains d.dname_span off)
      file.Ast.decls
  with
  | Some d ->
      Some
        (mk_hover (Printf.sprintf "%s %s" (decl_word d) d.dname) d.dname_span)
  | None -> (
      match
        List.find_opt (fun (_n, sp) -> contains sp off) (file_ty_refs file)
      with
      | Some (name, sp) ->
          let desc =
            match find_decl file name with
            | Some d -> Printf.sprintf "%s %s" (decl_word d) name
            | None -> name
          in
          Some (mk_hover desc sp)
      | None -> None)

let definition_at ~(uri : DocumentUri.t) ~(text : string) ~(file : Ast.file)
    (pos : Position.t) : Location.t option =
  let off = offset_of_position text pos in
  match List.find_opt (fun (_n, sp) -> contains sp off) (file_ty_refs file) with
  | None -> None
  | Some (name, _) -> (
      match find_decl file name with
      | Some d ->
          Some (Location.create ~uri ~range:(range_of_span d.dname_span))
      | None -> None)

(* The primitive keywords the lexer recognizes (kept in sync with
   [Tono_frontend]'s lowering); offered as completions alongside declared
   names. *)
let primitives =
  [
    "bool";
    "string";
    "bytes";
    "float";
    "timestamp";
    "date";
    "duration";
    "uuid";
    "i8";
    "i16";
    "i32";
    "i64";
    "u8";
    "u16";
    "u32";
    "u64";
  ]

let completions ~(file : Ast.file) : CompletionItem.t list =
  let decl_items =
    List.map
      (fun (d : Ast.decl) ->
        CompletionItem.create ~label:d.dname ~kind:CompletionItemKind.Struct
          ~detail:(decl_word d) ())
      file.Ast.decls
  in
  let prim_items =
    List.map
      (fun p ->
        CompletionItem.create ~label:p ~kind:CompletionItemKind.Keyword
          ~detail:"primitive" ())
      primitives
  in
  decl_items @ prim_items

(* --- symbols, references, rename, formatting --- *)

(* The name under the cursor, whether it sits on a declaration name or a type
   reference. The unit of navigation for references and rename. *)
let symbol_at ~(text : string) ~(file : Ast.file) (pos : Position.t) :
    string option =
  let off = offset_of_position text pos in
  match
    List.find_opt
      (fun (d : Ast.decl) -> contains d.dname_span off)
      file.Ast.decls
  with
  | Some d -> Some d.dname
  | None -> (
      match
        List.find_opt (fun (_n, sp) -> contains sp off) (file_ty_refs file)
      with
      | Some (name, _) -> Some name
      | None -> None)

(* Every span that names [name]: the declaration site (optional) plus all type
   references to it. The edit/highlight set behind references and rename. *)
let name_spans ~(include_decl : bool) (file : Ast.file) (name : string) :
    Span.span list =
  let decls =
    if include_decl then
      List.filter_map
        (fun (d : Ast.decl) ->
          if d.dname = name then Some d.dname_span else None)
        file.Ast.decls
    else []
  in
  let refs =
    List.filter_map
      (fun (n, sp) -> if n = name then Some sp else None)
      (file_ty_refs file)
  in
  decls @ refs

let references_at ~(uri : DocumentUri.t) ~(text : string) ~(file : Ast.file)
    ~(include_decl : bool) (pos : Position.t) : Location.t list =
  match symbol_at ~text ~file pos with
  | None -> []
  | Some name ->
      List.map
        (fun sp -> Location.create ~uri ~range:(range_of_span sp))
        (name_spans ~include_decl file name)

let rename_at ~(uri : DocumentUri.t) ~(text : string) ~(file : Ast.file)
    ~(new_name : string) (pos : Position.t) : WorkspaceEdit.t =
  match symbol_at ~text ~file pos with
  | None -> WorkspaceEdit.create ()
  | Some name ->
      let edits =
        List.map
          (fun sp ->
            TextEdit.create ~newText:new_name ~range:(range_of_span sp))
          (name_spans ~include_decl:true file name)
      in
      WorkspaceEdit.create ~changes:[ (uri, edits) ] ()

let leaf ~(kind : SymbolKind.t) ~(name : string) ~(span : Span.span) :
    DocumentSymbol.t =
  DocumentSymbol.create ~kind ~name ~range:(range_of_span span)
    ~selectionRange:(range_of_span span) ()

let member_symbols (d : Ast.decl) : DocumentSymbol.t list =
  match d.dkind with
  | Ast.DStruct { members; _ } ->
      List.map
        (fun m ->
          leaf ~kind:SymbolKind.Field ~name:m.Ast.mname ~span:m.mname_span)
        members
  | Ast.DEnum { cases } ->
      List.map
        (fun c ->
          leaf ~kind:SymbolKind.EnumMember ~name:c.Ast.cname ~span:c.cname_span)
        cases
  | Ast.DUnion { variants; _ } ->
      List.map
        (fun v ->
          leaf ~kind:SymbolKind.EnumMember ~name:v.Ast.vname ~span:v.vname_span)
        variants
  | Ast.DOp _ | Ast.DExt _ -> []

let decl_symbol_kind (d : Ast.decl) : SymbolKind.t =
  match d.dkind with
  | Ast.DStruct _ -> SymbolKind.Struct
  | Ast.DEnum _ -> SymbolKind.Enum
  | Ast.DUnion _ -> SymbolKind.Enum
  | Ast.DOp _ -> SymbolKind.Function
  | Ast.DExt _ -> SymbolKind.Interface

(* The document outline: one symbol per top-level shape, its members nested as
   children. Ranges use the name span (the surface AST carries no full-body
   span), which is enough for an editor's outline and breadcrumb. *)
let document_symbols ~(file : Ast.file) : DocumentSymbol.t list =
  List.map
    (fun (d : Ast.decl) ->
      DocumentSymbol.create ~kind:(decl_symbol_kind d) ~name:d.dname
        ~range:(range_of_span d.dname_span)
        ~selectionRange:(range_of_span d.dname_span)
        ~children:(member_symbols d) ())
    file.Ast.decls

(* End-of-document position, for a whole-file replacement range. *)
let end_position (text : string) : Position.t =
  let n = String.length text in
  let line = ref 0 and last_nl = ref (-1) in
  String.iteri
    (fun i c ->
      if c = '\n' then (
        incr line;
        last_nl := i))
    text;
  Position.create ~line:!line ~character:(n - !last_nl - 1)

(* Formatting reuses the frontend's canonical pretty-printer (the same engine
   behind `tono fmt`), so the editor and the CLI never disagree. A parse error
   yields no edit rather than a guess. *)
let formatting ~(text : string) : TextEdit.t list option =
  match Tono_frontend.format_source text with
  | Error _ -> None
  | Ok formatted ->
      let range =
        Range.create
          ~start:(Position.create ~line:0 ~character:0)
          ~end_:(end_position text)
      in
      Some [ TextEdit.create ~newText:formatted ~range ]
