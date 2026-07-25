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
  let code =
    m.Ast.mname ^ ": "
    ^ Printer.print_ty m.Ast.mtype
    ^ String.concat ""
        (List.map
           (fun t -> " " ^ Printer.print_trait t)
           (without_doc m.Ast.mtraits))
  in
  mk_hover ~markdown ~text ~code
    ~prose:(doc_of_traits m.Ast.mtraits)
    m.Ast.mname_span

(* The trait contracts surfaced on hover, offered after `@`, and expanded by
   signature help: one table, three consumers, so the documented keys can
   never drift between them. The frontend has no central trait registry (each
   checker pattern-matches its own keys); keep entries in step with the
   checkers that read them (check_http, check_constraints, protocol_http). *)
type trait_info = { ti_doc : string; ti_keys : (string * string) list }

let trait_registry : (string * trait_info) list =
  [
    ( "doc",
      {
        ti_doc =
          "Documentation carried into every generated SDK as the target \
           language's doc comment.";
        ti_keys = [ ("text", "string") ];
      } );
    ( "http",
      {
        ti_doc = "Binds the operation to an HTTP endpoint.";
        ti_keys = [ ("method", "\"GET\" | \"POST\" | ..."); ("path", "string") ];
      } );
    ( "range",
      {
        ti_doc = "Numeric bounds validated at the boundary (inclusive).";
        ti_keys = [ ("min", "int"); ("max", "int") ];
      } );
    ( "length",
      {
        ti_doc = "String length bounds validated at the boundary (inclusive).";
        ti_keys = [ ("min", "int"); ("max", "int") ];
      } );
    ( "rename",
      {
        ti_doc =
          "Overrides the idiomatic identifier for one language, e.g. \
           @rename(go: \"ID\"). The wire key is unchanged.";
        ti_keys = [ ("lang", "string") ];
      } );
    ( "deprecated",
      {
        ti_doc =
          "Marks the element deprecated in every generated SDK, with the given \
           note.";
        ti_keys = [ ("note", "string") ];
      } );
    ( "status",
      {
        ti_doc = "The HTTP status this error shape is discriminated by.";
        ti_keys = [ ("code", "int") ];
      } );
    ( "errorCode",
      {
        ti_doc =
          "The value matched against the payload's code field to select this \
           error shape.";
        ti_keys = [ ("value", "string") ];
      } );
    ( "errors",
      {
        ti_doc = "Declares the error shapes an operation can raise.";
        ti_keys = [ ("shapes", "name, ...") ];
      } );
    ( "async",
      {
        ti_doc =
          "Generates the asynchronous variant of the operation in targets that \
           distinguish it.";
        ti_keys = [];
      } );
    ( "discriminator",
      {
        ti_doc =
          "The union's tag field on the wire, e.g. @discriminator(\"kind\").";
        ti_keys = [ ("field", "string") ];
      } );
    ("retryable", { ti_doc = "Marks the error as safe to retry."; ti_keys = [] });
    ( "entries",
      {
        ti_doc =
          "Serializes a map as an array of key/value pairs, escaping keys that \
           cannot be object keys.";
        ti_keys = [];
      } );
  ]

(* The hover prose for a trait: its contract plus the keys it takes, rendered
   from the same registry entry. *)
let trait_doc_text (info : trait_info) : string =
  match info.ti_keys with
  | [] -> info.ti_doc
  | keys ->
      info.ti_doc ^ " Keys: "
      ^ String.concat ", " (List.map (fun (k, v) -> k ^ ": " ^ v) keys)
      ^ "."

let trait_docs : (string * string) list =
  List.map (fun (name, i) -> (name, trait_doc_text i)) trait_registry

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
      | Ast.DOp _ | Ast.DExt _ -> [])
    file.Ast.decls

let trait_hover ~markdown ~text (t : Ast.trait) : Hover.t option =
  match List.assoc_opt t.Ast.tname trait_docs with
  | None -> None
  | Some prose ->
      Some
        (mk_hover ~markdown ~text ~code:(Printer.print_trait t)
           ~prose:(Some prose) t.Ast.tspan)

(* Construct hover texts. The hook slot list is rendered from the exact table
   the typechecker enforces (Check_ext.hook_slots): the enumerated semantics
   must never grow a second hand-written copy. *)
let construct_doc (word : string) : string option =
  match word with
  | "struct" ->
      Some "A record shape: named members with types, serialized as an object."
  | "enum" ->
      Some
        "An open enumeration: strict on encode, lenient on decode (an unknown \
         value is carried, never a failure)."
  | "union" ->
      Some
        "A tagged sum: every variant carries a payload and travels internally \
         tagged by the discriminator field."
  | "op" ->
      Some
        "An operation: input and output shapes plus traits (transport, errors, \
         effect)."
  | "map" ->
      Some
        "A homogeneous map type, map[K]V. Keys that cannot be object keys can \
         escape to a pairs array with @entries."
  | "pub" -> Some "Exports the declaration across module boundaries."
  | "import" ->
      Some "Brings another module's declarations into dot-qualified scope."
  | "ext" ->
      Some
        "A bespoke extension point (hook, contract, or constraint), bound per \
         language to a file#symbol reference."
  | "hook" ->
      Some
        (Printf.sprintf
           "Fills a fixed lifecycle slot (%s), bound per language to a \
            file#symbol reference. Hooks take no signature."
           (String.concat ", " Check_ext.hook_slots))
  | "contract" ->
      Some
        "A bespoke function with a typed signature; emission is gated on a \
         conformance spec."
  | "constraint" ->
      Some "A bespoke validation predicate attached at the boundary."
  | _ -> None

(* Primitive and marker hover: the wire decisions that most surprise SDK
   consumers belong right under the cursor. *)
let primitive_doc (p : string) : string option =
  match p with
  | "i64" ->
      Some
        "64-bit signed integer. Travels as a string on the wire: JSON numbers \
         lose precision past 2^53."
  | "u64" -> Some "64-bit unsigned integer. Travels as a string on the wire."
  | "i8" | "i16" | "i32" ->
      Some
        "Signed integer. Arithmetic wraps at the width (two's complement); \
         division truncates toward zero."
  | "u8" | "u16" | "u32" ->
      Some "Unsigned integer. Arithmetic wraps modulo 2^width."
  | "float" ->
      Some
        "IEEE 754 double. There is no decimal type: money is integer minor \
         units."
  | "bool" -> Some "Boolean."
  | "string" -> Some "UTF-8 text."
  | "bytes" -> Some "Binary payload, base64 on the wire."
  | "timestamp" -> Some "An instant in time, carried as a branded string."
  | "date" -> Some "A calendar date, carried as a branded string."
  | "duration" -> Some "A span of time, carried as a branded string."
  | "uuid" -> Some "A UUID, carried as a branded string."
  | _ -> None

let nullable_doc =
  "Two-state nullability: the value is present or absent, nothing else. Absent \
   and null collapse; the default encoding omits the field."

(* Keyword, primitive, and marker hover resolve through the real lexer, so the
   word set can never drift from what the language accepts. hook, contract,
   and constraint are plain identifiers that only read as constructs right
   after `ext`. *)
let token_hover ~markdown ~text (off : int) : Hover.t option =
  let toks, _ = Lexer.tokenize text in
  let rec go after_ext = function
    | [] -> None
    | (t : Token.t) :: rest ->
        if contains t.span off then
          let found =
            match t.kind with
            | Token.KwStruct -> Some ("struct", construct_doc "struct")
            | Token.KwEnum -> Some ("enum", construct_doc "enum")
            | Token.KwUnion -> Some ("union", construct_doc "union")
            | Token.KwOp -> Some ("op", construct_doc "op")
            | Token.KwMap -> Some ("map", construct_doc "map")
            | Token.KwPub -> Some ("pub", construct_doc "pub")
            | Token.KwImport -> Some ("import", construct_doc "import")
            | Token.KwExt -> Some ("ext", construct_doc "ext")
            | Token.Prim p -> Some (p, primitive_doc p)
            | Token.Question -> Some ("?", Some nullable_doc)
            | Token.Ident w when after_ext -> Some (w, construct_doc w)
            | _ -> None
          in
          Option.bind found (fun (code, doc) ->
              Option.map
                (fun prose ->
                  mk_hover ~markdown ~text ~code ~prose:(Some prose) t.span)
                doc)
        else go (t.kind = Token.KwExt) rest
  in
  go false toks

(* Hover, most specific first: a declaration name, a member name, a type
   reference (which shows the full target declaration), a trait, then the
   token layer (keywords, primitives, the ? marker). *)
let hover_at ~(markdown : bool) ~(text : string) ~(file : Ast.file)
    (pos : Position.t) : Hover.t option =
  let off = offset_of_position text pos in
  match
    List.find_opt
      (fun (d : Ast.decl) -> contains d.dname_span off)
      file.Ast.decls
  with
  | Some d -> Some (decl_hover ~markdown ~text d d.dname_span)
  | None -> (
      match member_at file off with
      | Some m -> Some (member_hover ~markdown ~text m)
      | None -> (
          match
            List.find_opt (fun (_n, sp) -> contains sp off) (file_ty_refs file)
          with
          | Some (name, sp) -> (
              match find_decl file name with
              | Some d -> Some (decl_hover ~markdown ~text d sp)
              | None ->
                  Some (mk_hover ~markdown ~text ~code:name ~prose:None sp))
          | None -> (
              match
                List.find_opt
                  (fun (t : Ast.trait) -> contains t.Ast.tspan off)
                  (file_traits file)
              with
              | Some t -> trait_hover ~markdown ~text t
              | None -> token_hover ~markdown ~text off)))

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

let trait_items : CompletionItem.t list =
  List.map
    (fun (name, detail) ->
      CompletionItem.create ~label:name ~kind:CompletionItemKind.Property
        ~detail ())
    trait_docs

(* The lifecycle slots come from the checker's own table, so completion can
   only ever offer what the typechecker accepts. *)
let slot_items : CompletionItem.t list =
  List.map
    (fun s ->
      CompletionItem.create ~label:s ~kind:CompletionItemKind.EnumMember
        ~detail:"lifecycle slot" ())
    Check_ext.hook_slots

let is_ident_char c =
  (c >= 'a' && c <= 'z')
  || (c >= 'A' && c <= 'Z')
  || (c >= '0' && c <= '9')
  || c = '_'

(* The line prefix before the cursor picks the context: a trailing `@` wants a
   trait, `ext hook` wants a lifecycle slot, a trailing `:` wants a type
   (member types), and an open non-trait paren wants a type too (op input,
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
  let hook_context =
    match
      List.filter
        (fun w -> w <> "")
        (String.split_on_char ' ' (String.trim before))
    with
    | [ "ext"; "hook" ] | [ "pub"; "ext"; "hook" ] -> true
    | _ -> false
  in
  let type_context =
    if unclosed_parens > 0 then not (String.contains before '@')
    else
      let trimmed = String.trim before in
      String.length trimmed > 0 && trimmed.[String.length trimmed - 1] = ':'
  in
  if after_at then trait_items
  else if hook_context then slot_items
  else if type_context then prim_items @ decl_items file
  else decl_items file @ prim_items

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
  | Ast.DStruct { members; _ } ->
      List.map
        (fun m ->
          leaf ~text ~kind:SymbolKind.Field ~name:m.Ast.mname ~span:m.mname_span)
        members
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
  | Ok formatted ->
      let range =
        Range.create
          ~start:(Position.create ~line:0 ~character:0)
          ~end_:(end_position text)
      in
      Some [ TextEdit.create ~newText:formatted ~range ]

let range_in ~text span = range_of_span ~text span
