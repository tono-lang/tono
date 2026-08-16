(* The editor-facing analysis, kept free of any IO or JSON-RPC transport so the
   whole surface is unit-testable. Every answer is derived from the OCaml
   frontend (the single source of parsing and typechecking): diagnostics come
   from [Tono_frontend.compile], and navigation walks the surface [Ast] the
   parser produces. The server module is a thin stdio shell over this. *)

open Lsp.Types
module Ast = Tono_frontend.Ast
module Span = Tono_frontend.Span
module FDiag = Tono_frontend.Diagnostic
module Token = Tono_frontend.Token
module Lexer = Tono_frontend.Lexer
module Printer = Tono_frontend.Printer
module Check_ext = Tono_frontend.Check_ext
module Check_entries = Tono_frontend.Check_entries

(* The frontend's [Span.pos] is 1-based line/column counted in bytes (the lexer
   scans bytes); an LSP [Position] is 0-based line/character counted in UTF-16
   code units (the protocol default every client assumes). Both conversions
   decode the UTF-8 line around the position: a BMP scalar is one UTF-16 unit,
   an astral one is two, and a malformed byte counts as one unit so broken
   input still converges instead of shifting every later position. *)
let utf8_width (b : char) : int =
  let c = Char.code b in
  if c < 0x80 then 1
  else if c < 0xC0 then 1 (* stray continuation byte: count it alone *)
  else if c < 0xE0 then 2
  else if c < 0xF0 then 3
  else 4

let utf16_units_of_width (w : int) : int = if w = 4 then 2 else 1

(* UTF-16 column (0-based) of byte offset [stop] on the line starting at
   [line_start]. *)
let utf16_col (text : string) ~(line_start : int) ~(stop : int) : int =
  let n = String.length text in
  let rec go i units =
    if i >= stop || i >= n then units
    else
      let w = utf8_width text.[i] in
      go (i + w) (units + utf16_units_of_width w)
  in
  go line_start 0

let position_of_pos ~(text : string) (p : Span.pos) : Position.t =
  let character =
    utf16_col text ~line_start:(p.offset - (p.col - 1)) ~stop:p.offset
  in
  Position.create ~line:(p.line - 1) ~character

let range_of_span ~(text : string) (s : Span.span) : Range.t =
  Range.create
    ~start:(position_of_pos ~text s.start)
    ~end_:(position_of_pos ~text s.finish)

(* Byte offset of an LSP position inside [text]: walk to the start of the target
   line, then consume UTF-16 code units up to the character column, clamped to
   the end of that line. *)
let offset_of_position (text : string) (pos : Position.t) : int =
  let n = String.length text in
  let rec line_start i line =
    if line >= pos.line || i >= n then i
    else if text.[i] = '\n' then line_start (i + 1) (line + 1)
    else line_start (i + 1) line
  in
  let rec advance i units =
    if units >= pos.character || i >= n || text.[i] = '\n' then i
    else
      let w = utf8_width text.[i] in
      advance (i + w) (units + utf16_units_of_width w)
  in
  min n (advance (line_start 0 0) 0)

let parse (src : string) : Ast.file = fst (Tono_frontend.Parser.parse src)

(* Whether [s] can stand where a declaration name stands: exactly one
   identifier token and a clean lex. The lexer is the rule, so keywords lex as
   keyword tokens and primitives as [Prim] and both are rejected without a
   second reserved-word list. *)
let valid_identifier (s : string) : bool =
  match Lexer.tokenize s with
  | [ { Token.kind = Token.Ident id; _ }; { Token.kind = Token.Eof; _ } ], [] ->
      String.equal id s
  | _ -> false

let lsp_of_fdiags ~(text : string) (diags : FDiag.t list) : Diagnostic.t list =
  List.map
    (fun (d : FDiag.t) ->
      let severity =
        match d.severity with
        | FDiag.Error -> DiagnosticSeverity.Error
        | FDiag.Warning -> DiagnosticSeverity.Warning
      in
      let code = Option.map (fun c -> `String c) d.code in
      Diagnostic.create
        ~range:(range_of_span ~text d.span)
        ~severity ?code ~source:"tono" ~message:(`String d.message) ())
    diags

let lsp_diagnostics (src : string) : Diagnostic.t list =
  let _m, diags = Tono_frontend.compile src in
  lsp_of_fdiags ~text:src diags

(* --- surface AST navigation --- *)

let contains = Analysis_ext.contains

let decl_word (d : Ast.decl) : string =
  match d.dkind with
  | Ast.DStruct _ -> "struct"
  | Ast.DEnum _ -> "enum"
  | Ast.DUnion _ -> "union"
  | Ast.DOp _ -> "operation"
  | Ast.DExt { ekind = Ast.EHook; _ } -> "hook"
  | Ast.DExt { ekind = Ast.EContract; _ } -> "contract"
  | Ast.DExt { ekind = Ast.EConstraint; _ } -> "constraint"
  | Ast.DExt { ekind = Ast.EImpl; _ } -> "impl"
  | Ast.DExtLib _ -> "ext"
  | Ast.DTest _ -> "test"

(* A sub-span of [base] starting [skip] bytes in, [len] bytes long. Names are
   single-line tokens, so only the column and offset shift. *)
let name_sub_span (base : Span.span) ~(skip : int) ~(len : int) : Span.span =
  let s = base.Span.start in
  let start = { s with Span.col = s.col + skip; offset = s.offset + skip } in
  let finish =
    { start with Span.col = start.col + len; offset = start.offset + len }
  in
  { Span.start; finish }

(* Every named type reference inside a type, paired with the span of the name
   itself (a TName span covers its generic arguments too, and an edit over the
   full span would eat them). Primitives, type parameters, and error nodes
   carry no target so they are skipped. *)
let rec ty_refs (t : Ast.ty) (acc : (string * Span.span) list) :
    (string * Span.span) list =
  match t with
  | Ast.TName (name, args, span) ->
      let sp = name_sub_span span ~skip:0 ~len:(String.length name) in
      List.fold_left (fun a t -> ty_refs t a) ((name, sp) :: acc) args
  (* A module-qualified reference: the target lives in another file, so a local
     declaration lookup will miss, but hover still names it. *)
  | Ast.TQName (qual, name, args, span) ->
      let sp =
        name_sub_span span ~skip:0
          ~len:(String.length qual + 1 + String.length name)
      in
      List.fold_left
        (fun a t -> ty_refs t a)
        ((qual ^ "." ^ name, sp) :: acc)
        args
  | Ast.TList (t, _) | Ast.TNullable (t, _) -> ty_refs t acc
  | Ast.TMap (k, v, _) -> ty_refs v (ty_refs k acc)
  | Ast.TPrim _ | Ast.TError _ -> acc

(* The types written inside a declaration (member types, union payloads,
   operation input/output). Enum cases carry no types. *)
let rec decl_tys (d : Ast.decl) : Ast.ty list =
  match d.dkind with
  | Ast.DStruct { members; ops; _ } ->
      List.map (fun m -> m.Ast.mtype) members @ List.concat_map decl_tys ops
  | Ast.DUnion { variants; _ } ->
      List.filter_map (fun v -> v.Ast.vpayload) variants
  | Ast.DOp { pname = _; input; output; oimpl = _ } ->
      List.filter_map Fun.id [ input; output ]
  | Ast.DExt { esig; _ } -> (
      match esig with Some s -> [ s.Ast.esig_in; s.esig_out ] | None -> [])
  | Ast.DExtLib _ | Ast.DEnum _ | Ast.DTest _ -> []

let file_ty_refs (file : Ast.file) : (string * Span.span) list =
  List.concat_map
    (fun d -> List.concat_map (fun t -> ty_refs t []) (decl_tys d))
    file.Ast.decls

(* Every declaration reachable in the file: an entry declares its operations
   in its own body, and they are declarations like any other for hover,
   navigation, and the outline. *)
let all_decls (file : Ast.file) : Ast.decl list =
  List.concat_map
    (fun (d : Ast.decl) ->
      d :: (match d.Ast.dkind with Ast.DStruct { ops; _ } -> ops | _ -> []))
    file.Ast.decls

let find_decl (file : Ast.file) (name : string) : Ast.decl option =
  List.find_opt (fun (d : Ast.decl) -> d.dname = name) file.Ast.decls

(* --- hover content --- *)

let doc_of_traits (traits : Ast.trait list) : string option =
  List.find_map
    (fun (t : Ast.trait) ->
      if t.Ast.tname = "doc" then
        match t.Ast.targs with Ast.AString s :: _ -> Some s | _ -> None
      else None)
    traits

let without_doc (traits : Ast.trait list) : Ast.trait list =
  List.filter (fun (t : Ast.trait) -> t.Ast.tname <> "doc") traits

(* One hover shape everywhere: a code block (fenced as tono when the client
   renders markdown, bare otherwise) followed by optional prose. *)
let mk_hover ~(markdown : bool) ~(text : string) ~(code : string)
    ~(prose : string option) (span : Span.span) : Hover.t =
  let value =
    let body = if markdown then "```tono\n" ^ code ^ "\n```" else code in
    match prose with Some p -> body ^ "\n\n" ^ p | None -> body
  in
  let kind = if markdown then MarkupKind.Markdown else MarkupKind.PlainText in
  Hover.create
    ~contents:(`MarkupContent (MarkupContent.create ~kind ~value))
    ~range:(range_of_span ~text span) ()

(* Declaration hover: the full declaration pretty-printed by the canonical fmt
   printer (never a second renderer), minus its @doc trait, which reads better
   as prose under the code block. *)
let decl_hover ~markdown ~text (d : Ast.decl) (span : Span.span) : Hover.t =
  let shown = { d with Ast.dtraits = without_doc d.Ast.dtraits } in
  let code =
    String.trim (Printer.print_file { Ast.imports = []; decls = [ shown ] })
  in
  mk_hover ~markdown ~text ~code ~prose:(doc_of_traits d.Ast.dtraits) span

let member_at (file : Ast.file) (off : int) : Ast.member option =
  List.find_map
    (fun (d : Ast.decl) ->
      match d.Ast.dkind with
      | Ast.DStruct { members; _ } ->
          List.find_opt
            (fun (m : Ast.member) -> contains m.Ast.mname_span off)
            members
      | _ -> None)
    file.Ast.decls

(* Member hover: name, type, and traits in canonical form, then the member's
   own @doc prose. *)
let member_hover ~markdown ~text (m : Ast.member) : Hover.t =
  (* Rendered by the printer, so a field's sources and its selection table read
     on hover exactly as they are written in the file. *)
  let shown = { m with Ast.mtraits = without_doc m.Ast.mtraits } in
  let code = String.trim (Printer.print_member shown) in
  mk_hover ~markdown ~text ~code
    ~prose:(doc_of_traits m.Ast.mtraits)
    m.Ast.mname_span

let file_traits (file : Ast.file) : Ast.trait list =
  List.concat_map
    (fun (d : Ast.decl) ->
      d.Ast.dtraits
      @
      match d.Ast.dkind with
      | Ast.DStruct { members; _ } ->
          List.concat_map (fun (m : Ast.member) -> m.Ast.mtraits) members
      | Ast.DEnum { cases } ->
          List.concat_map (fun (c : Ast.enum_case) -> c.Ast.ctraits) cases
      | Ast.DUnion { variants; _ } ->
          List.concat_map
            (fun (v : Ast.union_variant) -> v.Ast.vtraits)
            variants
      | Ast.DOp _ | Ast.DExt _ | Ast.DExtLib _ | Ast.DTest _ -> [])
    (all_decls file)

let trait_hover ~markdown ~text (t : Ast.trait) : Hover.t option =
  match List.assoc_opt t.Ast.tname Hover_docs.trait_docs with
  | None -> None
  | Some prose ->
      Some
        (mk_hover ~markdown ~text ~code:(Printer.print_trait t)
           ~prose:(Some prose) t.Ast.tspan)

(* Keyword, primitive, and marker hover resolve through the real lexer, so the
   word set can never drift from what the language accepts. hook, contract,
   and constraint are plain identifiers that only read as constructs right
   after `ext`. *)
let token_hover ~markdown ~text (off : int) : Hover.t option =
  let toks, _ = Lexer.tokenize text in
  (* "raw" reads as a construct anywhere inside an ext declaration (it follows
     the name, not the kind word), "match" only right after the "=" that opens
     a field's selection table. *)
  let next_in_ext (k : Token.kind) (in_ext : bool) =
    match k with Token.KwExt -> true | Token.LBrace -> false | _ -> in_ext
  in
  let rec go in_ext after_eq = function
    | [] -> None
    | (t : Token.t) :: rest ->
        if contains t.span off then
          let found =
            match t.kind with
            | Token.KwStruct ->
                Some ("struct", Hover_docs.construct_doc "struct")
            | Token.KwEnum -> Some ("enum", Hover_docs.construct_doc "enum")
            | Token.KwUnion -> Some ("union", Hover_docs.construct_doc "union")
            | Token.KwOp -> Some ("op", Hover_docs.construct_doc "op")
            | Token.KwMap -> Some ("map", Hover_docs.construct_doc "map")
            | Token.KwPub -> Some ("pub", Hover_docs.construct_doc "pub")
            | Token.KwImport ->
                Some ("import", Hover_docs.construct_doc "import")
            | Token.KwExt -> Some ("ext", Hover_docs.construct_doc "ext")
            | Token.Prim p -> Some (p, Hover_docs.primitive_doc p)
            | Token.Question -> Some ("?", Some Hover_docs.nullable_doc)
            | Token.Ident "match" when after_eq ->
                Some ("match", Hover_docs.construct_doc "match")
            | Token.Ident w when in_ext -> Some (w, Hover_docs.construct_doc w)
            | _ -> None
          in
          Option.bind found (fun (code, doc) ->
              Option.map
                (fun prose ->
                  mk_hover ~markdown ~text ~code ~prose:(Some prose) t.span)
                doc)
        else go (next_in_ext t.kind in_ext) (t.kind = Token.Eq) rest
  in
  go false false toks

(* The ext library block's contextual words (extern, call:, sync, ...) and
   the leading-dot `.request` reference, resolved by their position in the
   token stream. Checked before the trait layer because `.request` sits
   inside a trait's argument list. *)
let ext_word_hover ~markdown ~text (off : int) : Hover.t option =
  let toks, _ = Lexer.tokenize text in
  match Analysis_ext.word_at toks off with
  | None -> None
  | Some w ->
      let t = List.find (fun (t : Token.t) -> contains t.span off) toks in
      Option.map
        (fun prose ->
          mk_hover ~markdown ~text ~code:w ~prose:(Some prose) t.span)
        (Hover_docs.construct_doc w)

(* Hover, most specific first: a declaration name, a member name, an extern
   or handle name, a type reference (which shows the full target
   declaration), an ext block word, a trait, then the token layer (keywords,
   primitives, the ? marker). *)
let hover_at ~(markdown : bool) ~(text : string) ~(file : Ast.file)
    (pos : Position.t) : Hover.t option =
  let off = offset_of_position text pos in
  match
    List.find_opt
      (fun (d : Ast.decl) -> contains d.dname_span off)
      (all_decls file)
  with
  | Some d -> Some (decl_hover ~markdown ~text d d.dname_span)
  | None -> (
      match member_at file off with
      | Some m -> Some (member_hover ~markdown ~text m)
      | None -> (
          (* An extern or opaque handle declared inside an ext block, shown
             as the fmt printer writes it, like any declaration. *)
          match Analysis_ext.named_at file off with
          | Some (code, sp) ->
              Some (mk_hover ~markdown ~text ~code ~prose:None sp)
          | None -> (
              match
                List.find_opt
                  (fun (_n, sp) -> contains sp off)
                  (file_ty_refs file)
              with
              | Some (name, sp) -> (
                  match find_decl file name with
                  | Some d -> Some (decl_hover ~markdown ~text d sp)
                  | None ->
                      Some (mk_hover ~markdown ~text ~code:name ~prose:None sp))
              | None -> (
                  match ext_word_hover ~markdown ~text off with
                  | Some h -> Some h
                  | None -> (
                      match
                        List.find_opt
                          (fun (t : Ast.trait) -> contains t.Ast.tspan off)
                          (file_traits file)
                      with
                      | Some t -> trait_hover ~markdown ~text t
                      | None -> token_hover ~markdown ~text off)))))

let definition_at ~(uri : DocumentUri.t) ~(text : string) ~(file : Ast.file)
    (pos : Position.t) : Location.t option =
  let off = offset_of_position text pos in
  match List.find_opt (fun (_n, sp) -> contains sp off) (file_ty_refs file) with
  | None -> None
  | Some (name, _) -> (
      match find_decl file name with
      | Some d ->
          Some (Location.create ~uri ~range:(range_of_span ~text d.dname_span))
      | None -> None)

(* The primitive keywords, read from the lexer's own table so completion can
   never offer a name the lexer will not recognize. *)
let primitives = Lexer.prims

let decl_items (file : Ast.file) : CompletionItem.t list =
  List.map
    (fun (d : Ast.decl) ->
      CompletionItem.create ~label:d.dname ~kind:CompletionItemKind.Struct
        ~detail:(decl_word d) ())
    file.Ast.decls

let prim_items : CompletionItem.t list =
  List.map
    (fun p ->
      CompletionItem.create ~label:p ~kind:CompletionItemKind.Keyword
        ~detail:"primitive" ())
    primitives

(* Declaration-starter keywords, offered where a declaration can begin. The
   prose is the hover's construct text, so the two never drift. *)
let keyword_item (word : string) : CompletionItem.t =
  match Hover_docs.construct_doc word with
  | Some doc ->
      CompletionItem.create ~label:word ~kind:CompletionItemKind.Keyword
        ~detail:"keyword" ~documentation:(`String doc) ()
  | None ->
      CompletionItem.create ~label:word ~kind:CompletionItemKind.Keyword
        ~detail:"keyword" ()

let trait_items : CompletionItem.t list =
  List.map
    (fun (name, detail) ->
      CompletionItem.create ~label:name ~kind:CompletionItemKind.Property
        ~detail ())
    Hover_docs.trait_docs

(* The extension kinds a declared extension can validly be. `hook` still
   parses (see Check_ext, which rejects it with a migration message), but it
   is never a valid kind to write, so completion never offers it. *)
let ext_kind_items : CompletionItem.t list =
  List.map
    (fun (k, detail) ->
      CompletionItem.create ~label:k ~kind:CompletionItemKind.Keyword ~detail ())
    [
      ("contract", "bespoke contract");
      ("constraint", "bespoke constraint");
      ("impl", "bespoke operation implementation");
    ]

(* The words of an ext library block, offered by frame: what may open a
   declaration in an ext body, the fields (as `word:`) and markers of a
   language block, the lone `extern` of a handle body. The prose is the
   hover's, so the two never drift. *)
let ext_word_item ?(suffix = "") (word : string) : CompletionItem.t =
  let documentation =
    Option.map (fun d -> `String d) (Hover_docs.construct_doc word)
  in
  CompletionItem.create ~label:word ~kind:CompletionItemKind.Keyword
    ~detail:"ext block" ~insertText:(word ^ suffix) ?documentation ()

let ext_frame_items (frame : Analysis_ext.frame) : CompletionItem.t list option
    =
  let open Analysis_ext in
  match frame with
  | Ext ->
      Some
        (List.map ext_word_item
           ("struct" :: Tono_frontend.Ext_lib_vocab.block_words))
  | Type -> Some [ ext_word_item "extern" ]
  | Lang ->
      Some
        (List.map
           (ext_word_item ~suffix:": ")
           Tono_frontend.Ext_lib_vocab.lang_fields
        @ List.map ext_word_item Tono_frontend.Ext_lib_vocab.lang_markers)
  | Extern | Other -> None

(* The @str:: catalog, offered after the separator. *)
let str_catalog_items : CompletionItem.t list =
  List.map
    (fun name ->
      CompletionItem.create ~label:name ~kind:CompletionItemKind.Function
        ~detail:"@str:: transform" ())
    Check_entries.str_transforms

(* The operations an entry declares: what an `ext impl` can name. The bare name
   is the normal form; the qualified one disambiguates. *)
let entry_op_items (file : Ast.file) : CompletionItem.t list =
  List.concat_map
    (fun (d : Ast.decl) ->
      match d.Ast.dkind with
      | Ast.DStruct { ops; _ } ->
          List.concat_map
            (fun (o : Ast.decl) ->
              [
                CompletionItem.create ~label:o.Ast.dname
                  ~kind:CompletionItemKind.Method
                  ~detail:("operation of " ^ d.Ast.dname)
                  ();
                CompletionItem.create
                  ~label:(d.Ast.dname ^ "." ^ o.Ast.dname)
                  ~kind:CompletionItemKind.Method ~detail:"qualified operation"
                  ();
              ])
            ops
      | _ -> [])
    file.Ast.decls

(* The fields of the entry or config the cursor sits in: the only names a
   `.field` reference can resolve to. The enclosing declaration is the last one
   whose name starts before the cursor, since the surface AST carries no
   body span. *)
let enclosing_fields (file : Ast.file) (off : int) : Ast.member list =
  List.fold_left
    (fun acc (d : Ast.decl) ->
      if d.Ast.dname_span.Span.start.offset > off then acc
      else
        match d.Ast.dkind with Ast.DStruct { members; _ } -> members | _ -> [])
    [] file.Ast.decls

let field_items (file : Ast.file) (off : int) : CompletionItem.t list =
  List.map
    (fun (m : Ast.member) ->
      CompletionItem.create ~label:m.Ast.mname ~kind:CompletionItemKind.Field
        ~detail:(Printer.print_ty m.Ast.mtype)
        ())
    (enclosing_fields file off)

let is_ident_char c =
  (c >= 'a' && c <= 'z')
  || (c >= 'A' && c <= 'Z')
  || (c >= '0' && c <= '9')
  || c = '_'

(* The line prefix before the cursor picks the context: a trailing `@` wants a
   trait, a trailing `:` wants a type (member types), and an open non-trait
   paren wants a type too (op input,
   variant payload). Anything else gets the flat list. *)
let completions ~(text : string) ~(file : Ast.file) (pos : Position.t) :
    CompletionItem.t list =
  let off = offset_of_position text pos in
  let rec bol i =
    if i <= 0 then 0 else if text.[i - 1] = '\n' then i else bol (i - 1)
  in
  let start = bol off in
  let prefix = String.sub text start (off - start) in
  (* Strip the partially-typed identifier: what precedes it decides. *)
  let stem_end =
    let rec back i =
      if i > 0 && is_ident_char prefix.[i - 1] then back (i - 1) else i
    in
    back (String.length prefix)
  in
  let before = String.sub prefix 0 stem_end in
  let unclosed_parens =
    String.fold_left
      (fun d c ->
        if c = '(' then d + 1 else if c = ')' then max 0 (d - 1) else d)
      0 before
  in
  let after_at =
    String.length before > 0 && before.[String.length before - 1] = '@'
  in
  let words =
    List.filter
      (fun w -> w <> "")
      (String.split_on_char ' ' (String.trim before))
  in
  let ext_kind_context =
    match words with [ "ext" ] | [ "pub"; "ext" ] -> true | _ -> false
  in
  let impl_context =
    match words with
    | [ "ext"; "impl" ] | [ "pub"; "ext"; "impl" ] -> true
    | _ -> false
  in
  (* `@str::` and friends: the separator is not an identifier character, so the
     stem stops at it and the catalog name is what comes next. *)
  let catalog_context =
    let n = String.length before in
    n >= 4
    && String.sub before (n - 2) 2 = "::"
    &&
    let rec back i =
      if i > 0 && is_ident_char before.[i - 1] then back (i - 1) else i
    in
    let name_start = back (n - 2) in
    name_start > 0 && before.[name_start - 1] = '@' && name_start < n - 2
  in
  (* A leading `.` opens a field reference: inside @env(, a match subject, an
     arm value, a @header value. A dot right after an identifier is a path or a
     module qualifier instead, and is left alone. *)
  let ref_context =
    let n = String.length before in
    n > 0
    && before.[n - 1] = '.'
    && (n = 1 || not (is_ident_char before.[n - 2]))
  in
  (* `ns.` right after an identifier: a call site naming an ext's externs or a
     handle field's methods. Anything else after such a dot is a path or a
     module qualifier and gets no list. *)
  let call_ns =
    let n = String.length before in
    if n > 1 && before.[n - 1] = '.' && is_ident_char before.[n - 2] then
      let rec back i =
        if i > 0 && is_ident_char before.[i - 1] then back (i - 1) else i
      in
      Some (String.sub before (back (n - 1)) (n - 1 - back (n - 1)))
    else None
  in
  let ext_frame =
    match call_ns with
    | Some _ -> None
    | None ->
        ext_frame_items (Analysis_ext.frame_at (fst (Lexer.tokenize text)) off)
  in
  let type_context =
    if unclosed_parens > 0 then not (String.contains before '@')
    else
      let trimmed = String.trim before in
      String.length trimmed > 0 && trimmed.[String.length trimmed - 1] = ':'
  in
  if catalog_context then str_catalog_items
  else if after_at then trait_items
  else if Option.is_some call_ns then
    Analysis_ext.call_items
      ~fields:(enclosing_fields file off)
      file (Option.get call_ns)
  else if ext_kind_context then ext_kind_items
  else if impl_context then entry_op_items file
  else if ref_context then field_items file off
  else if type_context then (keyword_item "map" :: prim_items) @ decl_items file
  else if Option.is_some ext_frame then Option.get ext_frame
  else
    (* Keywords only where a declaration can begin: a blank prefix offers the
       starters, a lone `pub` offers what may follow it. Anywhere else they
       would be noise next to the declared shapes. *)
    let starters =
      match words with
      | [] -> [ "pub"; "struct"; "enum"; "union"; "op"; "import"; "ext" ]
      | [ "pub" ] -> [ "struct"; "enum"; "union"; "op"; "ext" ]
      | _ -> []
    in
    List.map keyword_item starters @ decl_items file @ prim_items

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
        (fun sp -> Location.create ~uri ~range:(range_of_span ~text sp))
        (name_spans ~include_decl file name)

let rename_at ~(uri : DocumentUri.t) ~(text : string) ~(file : Ast.file)
    ~(new_name : string) (pos : Position.t) : WorkspaceEdit.t =
  match symbol_at ~text ~file pos with
  | None -> WorkspaceEdit.create ()
  | Some name ->
      let edits =
        List.map
          (fun sp ->
            TextEdit.create ~newText:new_name ~range:(range_of_span ~text sp))
          (name_spans ~include_decl:true file name)
      in
      WorkspaceEdit.create ~changes:[ (uri, edits) ] ()

let leaf ~(text : string) ~(kind : SymbolKind.t) ~(name : string)
    ~(span : Span.span) : DocumentSymbol.t =
  DocumentSymbol.create ~kind ~name ~range:(range_of_span ~text span)
    ~selectionRange:(range_of_span ~text span) ()

let member_symbols ~(text : string) (d : Ast.decl) : DocumentSymbol.t list =
  match d.dkind with
  | Ast.DStruct { members; ops; _ } ->
      List.map
        (fun m ->
          leaf ~text ~kind:SymbolKind.Field ~name:m.Ast.mname ~span:m.mname_span)
        members
      @ List.map
          (fun (o : Ast.decl) ->
            leaf ~text ~kind:SymbolKind.Method ~name:o.Ast.dname
              ~span:o.Ast.dname_span)
          ops
  | Ast.DEnum { cases } ->
      List.map
        (fun c ->
          leaf ~text ~kind:SymbolKind.EnumMember ~name:c.Ast.cname
            ~span:c.cname_span)
        cases
  | Ast.DUnion { variants; _ } ->
      List.map
        (fun v ->
          leaf ~text ~kind:SymbolKind.EnumMember ~name:v.Ast.vname
            ~span:v.vname_span)
        variants
  | Ast.DExtLib { body; _ } ->
      Analysis_ext.symbols ~range:(range_of_span ~text) body
  | Ast.DOp _ | Ast.DExt _ | Ast.DTest _ -> []

let decl_symbol_kind (d : Ast.decl) : SymbolKind.t =
  match d.dkind with
  | Ast.DStruct _ -> SymbolKind.Struct
  | Ast.DEnum _ -> SymbolKind.Enum
  | Ast.DUnion _ -> SymbolKind.Enum
  | Ast.DOp _ -> SymbolKind.Function
  | Ast.DExt _ -> SymbolKind.Interface
  | Ast.DExtLib _ -> SymbolKind.Interface
  | Ast.DTest _ -> SymbolKind.Function

(* The document outline: one symbol per top-level shape, its members nested as
   children. Ranges use the name span (the surface AST carries no full-body
   span), which is enough for an editor's outline and breadcrumb. *)
let document_symbols ~(text : string) ~(file : Ast.file) : DocumentSymbol.t list
    =
  List.map
    (fun (d : Ast.decl) ->
      DocumentSymbol.create ~kind:(decl_symbol_kind d) ~name:d.dname
        ~range:(range_of_span ~text d.dname_span)
        ~selectionRange:(range_of_span ~text d.dname_span)
        ~children:(member_symbols ~text d) ())
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
  Position.create ~line:!line
    ~character:(utf16_col text ~line_start:(!last_nl + 1) ~stop:n)

(* Formatting reuses the frontend's canonical pretty-printer (the same engine
   behind `tono fmt`), so the editor and the CLI never disagree. A parse error
   yields no edit rather than a guess. *)
let formatting ~(text : string) : TextEdit.t list option =
  match Tono_frontend.format_source text with
  | Error _ -> None
  | Ok formatted when String.equal formatted text ->
      (* Already canonical: an identity edit would make clients mark the
         buffer modified for nothing. *)
      Some []
  | Ok formatted ->
      let range =
        Range.create
          ~start:(Position.create ~line:0 ~character:0)
          ~end_:(end_position text)
      in
      Some [ TextEdit.create ~newText:formatted ~range ]

let range_in ~text span = range_of_span ~text span
