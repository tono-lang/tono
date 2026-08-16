(* The request layer built on top of [Analysis]: workspace projects (module
   resolution with project context, cross-file navigation, workspace rename
   and symbols), the quick fixes derived from diagnostic codes, and signature
   help for trait arguments. Pure like [Analysis]: the server feeds it file
   contents and round-trips opaque document ids (URIs). *)

open Lsp.Types
module Ast = Tono_frontend.Ast
module Span = Tono_frontend.Span
module FDiag = Tono_frontend.Diagnostic
module Check_ext = Tono_frontend.Check_ext

let contains = Analysis.contains
let range_of_span = Analysis.range_of_span
let offset_of_position = Analysis.offset_of_position
let name_sub_span = Analysis.name_sub_span
let decl_tys = Analysis.decl_tys
let find_decl = Analysis.find_decl
let decl_symbol_kind = Analysis.decl_symbol_kind
let lsp_of_fdiags = Analysis.lsp_of_fdiags
let file_traits = Analysis.file_traits
let trait_registry = Hover_docs.trait_registry

(* --- workspace projects --- *)

module Modules = Tono_frontend.Modules
module Error_codes = Tono_frontend.Error_codes

(* One file of a project: its dotted module name, an opaque document id the
   server round-trips (a URI string), and the parsed source. *)
type project_entry = {
  pe_module : string;
  pe_id : string;
  pe_text : string;
  pe_file : Ast.file;
  pe_parse_diags : FDiag.t list;
}

let project_entry ~module_ ~id ~text : project_entry =
  let file, pdiags = Tono_frontend.Parser.parse text in
  {
    pe_module = module_;
    pe_id = id;
    pe_text = text;
    pe_file = file;
    pe_parse_diags = pdiags;
  }

type project = {
  entries : project_entry list;
  index : Modules.index;
  index_diags : (string * FDiag.t) list;
}

let build_project (entries : project_entry list) : project =
  let index, index_diags =
    Modules.build_attributed
      (List.map (fun e -> (e.pe_module, e.pe_file)) entries)
  in
  { entries; index; index_diags }

let project_find (p : project) (module_ : string) : project_entry option =
  List.find_opt (fun e -> e.pe_module = module_) p.entries

(* Raw frontend diagnostics for one module: parse, the index diagnostics
   attributed to it, and its lower+typecheck against the project index (the
   same pipeline the CLI's compile-dir runs, never a second resolver). *)
let project_module_fdiags (p : project) (module_ : string) : FDiag.t list =
  match project_find p module_ with
  | None -> []
  | Some e ->
      let _m, own =
        Tono_frontend.check_project_module p.index ~name:module_ e.pe_file
      in
      let index_own =
        List.filter_map
          (fun (n, d) -> if n = module_ then Some d else None)
          p.index_diags
      in
      FDiag.sort (e.pe_parse_diags @ index_own @ own)

let project_diagnostics (p : project) ~(module_ : string) : Diagnostic.t list =
  match project_find p module_ with
  | None -> []
  | Some e -> lsp_of_fdiags ~text:e.pe_text (project_module_fdiags p module_)

(* Import qualifier -> target module for one entry, through the frontend's own
   alias/segment mapping. *)
let import_target (e : project_entry) (qual : string) : string option =
  List.find_map
    (fun (i : Ast.import) ->
      if Modules.qualifier_of i = qual then Some (Modules.target_of i) else None)
    e.pe_file.Ast.imports

(* The transitive import closure of a module (itself included), the unit a
   cached check result depends on. *)
let module_closure (p : project) (module_ : string) : string list =
  let rec go seen = function
    | [] -> seen
    | m :: rest when List.mem m seen -> go seen rest
    | m :: rest ->
        let deps =
          match project_find p m with
          | None -> []
          | Some e -> List.map Modules.target_of e.pe_file.Ast.imports
        in
        go (m :: seen) (deps @ rest)
  in
  List.sort compare (go [] [ module_ ])

(* A content key over the module and everything it can see: equal keys mean an
   identical check result, so callers can reuse a cached one. *)
let module_check_key (p : project) ~(module_ : string) : string =
  Digest.to_hex
    (Digest.string
       (String.concat "\x00"
          (List.map
             (fun m ->
               match project_find p m with
               | Some e -> m ^ "\x01" ^ e.pe_text
               | None -> m)
             (module_closure p module_))))

(* The declared symbol under the cursor as (declaring module, name): the
   declaration name itself, a local reference, or a qualified reference
   resolved through this module's imports. *)
let rec ty_symbol (e : project_entry) (off : int) (t : Ast.ty) :
    (string * string) option =
  let first_arg args = List.find_map (ty_symbol e off) args in
  match t with
  | Ast.TName (name, args, span) -> (
      match first_arg args with
      | Some s -> Some s
      | None ->
          let sp = name_sub_span span ~skip:0 ~len:(String.length name) in
          if contains sp off then Some (e.pe_module, name) else None)
  | Ast.TQName (qual, name, args, span) -> (
      match first_arg args with
      | Some s -> Some s
      | None ->
          let sp =
            name_sub_span span ~skip:0
              ~len:(String.length qual + 1 + String.length name)
          in
          if contains sp off then
            Option.map (fun target -> (target, name)) (import_target e qual)
          else None)
  | Ast.TList (t, _) | Ast.TNullable (t, _) -> ty_symbol e off t
  | Ast.TMap (k, v, _) -> (
      match ty_symbol e off k with
      | Some s -> Some s
      | None -> ty_symbol e off v)
  | Ast.TPrim _ | Ast.TError _ -> None

let project_symbol_at (p : project) ~(module_ : string) (pos : Position.t) :
    (string * string) option =
  match project_find p module_ with
  | None -> None
  | Some e -> (
      let off = offset_of_position e.pe_text pos in
      match
        List.find_opt
          (fun (d : Ast.decl) -> contains d.dname_span off)
          e.pe_file.Ast.decls
      with
      | Some d -> Some (module_, d.dname)
      | None ->
          List.find_map
            (fun d -> List.find_map (ty_symbol e off) (decl_tys d))
            e.pe_file.Ast.decls)

(* The declaration site of (module, name), as (document id, range). *)
let project_decl_location (p : project) ~(module_ : string) ~(name : string) :
    (string * Range.t) option =
  Option.bind (project_find p module_) (fun e ->
      Option.map
        (fun (d : Ast.decl) ->
          (e.pe_id, range_of_span ~text:e.pe_text d.dname_span))
        (find_decl e.pe_file name))

(* Every span in one entry that references (module_, name): local references
   when the entry is the declaring module, qualified references resolved to it
   otherwise. Spans cover the name only, so an edit never eats a qualifier or
   a generic argument list. *)
let rec ty_occurrences (e : project_entry) ~(module_ : string) ~(name : string)
    (t : Ast.ty) (acc : Span.span list) : Span.span list =
  let into_args args acc =
    List.fold_left (fun a t -> ty_occurrences e ~module_ ~name t a) acc args
  in
  match t with
  | Ast.TName (n, args, span) ->
      let acc =
        if e.pe_module = module_ && n = name then
          name_sub_span span ~skip:0 ~len:(String.length n) :: acc
        else acc
      in
      into_args args acc
  | Ast.TQName (q, n, args, span) ->
      let acc =
        if n = name && import_target e q = Some module_ then
          name_sub_span span ~skip:(String.length q + 1) ~len:(String.length n)
          :: acc
        else acc
      in
      into_args args acc
  | Ast.TList (t, _) | Ast.TNullable (t, _) ->
      ty_occurrences e ~module_ ~name t acc
  | Ast.TMap (k, v, _) ->
      ty_occurrences e ~module_ ~name v (ty_occurrences e ~module_ ~name k acc)
  | Ast.TPrim _ | Ast.TError _ -> acc

let project_occurrences (p : project) ~(module_ : string) ~(name : string)
    ~(include_decl : bool) : (string * string * Span.span) list =
  List.concat_map
    (fun (e : project_entry) ->
      let decl_spans =
        if e.pe_module = module_ && include_decl then
          List.filter_map
            (fun (d : Ast.decl) ->
              if d.dname = name then Some d.dname_span else None)
            e.pe_file.Ast.decls
        else []
      in
      let ref_spans =
        List.concat_map
          (fun d ->
            List.fold_left
              (fun a t -> ty_occurrences e ~module_ ~name t a)
              [] (decl_tys d))
          e.pe_file.Ast.decls
      in
      List.map (fun sp -> (e.pe_id, e.pe_text, sp)) (decl_spans @ ref_spans))
    p.entries

type rename_outcome =
  | Renamed of (string * TextEdit.t list) list
  | Refused of string
  | NotASymbol

(* Workspace rename: every file referencing the symbol is edited; a rename
   that would collide with an existing declaration in the target module is
   refused with the reason. *)
let project_rename (p : project) ~(module_ : string) (pos : Position.t)
    ~(new_name : string) : rename_outcome =
  if not (Analysis.valid_identifier new_name) then
    Refused
      (Printf.sprintf
         "'%s' is not a valid declaration name (one identifier; keywords and \
          primitive names are reserved)"
         new_name)
  else
    match project_symbol_at p ~module_ pos with
    | None -> NotASymbol
    | Some (target_module, name) -> (
        let collides =
          match project_find p target_module with
          | Some e ->
              List.exists
                (fun (d : Ast.decl) -> d.dname = new_name)
                e.pe_file.Ast.decls
          | None -> false
        in
        if collides then
          Refused
            (Printf.sprintf
               "a declaration named '%s' already exists in module %s" new_name
               target_module)
        else
          match
            project_occurrences p ~module_:target_module ~name
              ~include_decl:true
          with
          | [] -> NotASymbol
          | occ ->
              let grouped =
                List.fold_left
                  (fun acc (id, text, sp) ->
                    let edit =
                      TextEdit.create ~newText:new_name
                        ~range:(range_of_span ~text sp)
                    in
                    match List.assoc_opt id acc with
                    | Some edits ->
                        (id, edit :: edits) :: List.remove_assoc id acc
                    | None -> (id, [ edit ]) :: acc)
                  [] occ
              in
              Renamed grouped)

(* Project-wide symbol search: case-insensitive substring over every declared
   shape and operation. *)
let project_symbols (p : project) ~(query : string) :
    (string * SymbolKind.t * string * Range.t) list =
  let q = String.lowercase_ascii query in
  let matches name =
    let n = String.lowercase_ascii name in
    let ln = String.length n and lq = String.length q in
    let rec go i = i + lq <= ln && (String.sub n i lq = q || go (i + 1)) in
    lq = 0 || go 0
  in
  List.concat_map
    (fun (e : project_entry) ->
      List.filter_map
        (fun (d : Ast.decl) ->
          if matches d.dname then
            Some
              ( d.dname,
                decl_symbol_kind d,
                e.pe_id,
                range_of_span ~text:e.pe_text d.dname_span )
          else None)
        e.pe_file.Ast.decls)
    p.entries

(* --- code actions --- *)

let last_segment (m : string) : string =
  match List.rev (String.split_on_char '.' m) with x :: _ -> x | [] -> m

(* Quick fixes derived from diagnostic codes, never from message text: the
   code identifies the remedy and the span carries the data. *)
let project_code_actions (p : project) ~(module_ : string) ~(range : Range.t) :
    (string * (string * TextEdit.t) list) list =
  match project_find p module_ with
  | None -> []
  | Some e ->
      let start_off = offset_of_position e.pe_text range.start in
      let end_off = offset_of_position e.pe_text range.end_ in
      let overlaps (s : Span.span) =
        s.Span.start.offset <= end_off && start_off <= s.finish.offset
      in
      let span_text (s : Span.span) =
        String.sub e.pe_text s.Span.start.offset
          (s.finish.offset - s.Span.start.offset)
      in
      (* The canonical import position is where the formatter puts imports:
         after the last existing one, else the top of the file. *)
      let import_edit target =
        let insert_at, prefix, suffix =
          match List.rev e.pe_file.Ast.imports with
          | (last : Ast.import) :: _ ->
              (Position.create ~line:last.ispan.finish.line ~character:0, "", "")
          | [] -> (Position.create ~line:0 ~character:0, "", "\n")
        in
        ignore prefix;
        TextEdit.create
          ~newText:("import " ^ target ^ "\n" ^ suffix)
          ~range:(Range.create ~start:insert_at ~end_:insert_at)
      in
      List.concat_map
        (fun (d : FDiag.t) ->
          if not (overlaps d.span) then []
          else if d.code = Some Error_codes.unknown_import then
            let referenced = span_text d.span in
            let qual =
              match String.index_opt referenced '.' with
              | Some i -> String.sub referenced 0 i
              | None -> referenced
            in
            List.filter_map
              (fun (other : project_entry) ->
                if
                  other.pe_module <> module_
                  && last_segment other.pe_module = qual
                then
                  Some
                    ( "import " ^ other.pe_module,
                      [ (e.pe_id, import_edit other.pe_module) ] )
                else None)
              p.entries
          else [])
        (project_module_fdiags p module_)

(* --- signature help --- *)

(* Signature help inside a trait's argument list, from the same registry as
   trait hover and completion. *)
let signature_help ~(text : string) ~(file : Ast.file) (pos : Position.t) :
    SignatureHelp.t option =
  let off = offset_of_position text pos in
  (* A trait's span covers only "@name"; its argument list follows in the
     source. The cursor is inside the arguments when it sits after the opening
     paren that follows the name and before the paren that closes it. *)
  let inside (t : Ast.trait) =
    let fin = t.Ast.tspan.finish.offset in
    let n = String.length text in
    let rec skip_blank i =
      if i < n && (text.[i] = ' ' || text.[i] = '\t') then skip_blank (i + 1)
      else i
    in
    let lp = skip_blank fin in
    fin <= off && lp < n
    && text.[lp] = '('
    && off > lp
    &&
    let rec still_open i depth =
      if i >= off || i >= n then true
      else
        match text.[i] with
        | '(' -> still_open (i + 1) (depth + 1)
        | ')' -> if depth = 1 then false else still_open (i + 1) (depth - 1)
        | _ -> still_open (i + 1) depth
    in
    still_open lp 0
  in
  match List.find_opt inside (file_traits file) with
  | None -> None
  | Some t -> (
      match List.assoc_opt t.Ast.tname trait_registry with
      | None | Some { Hover_docs.ti_keys = []; _ } -> None
      | Some info ->
          let params =
            List.map
              (fun (k, shape) -> k ^ ": " ^ shape)
              info.Hover_docs.ti_keys
          in
          let label =
            "@" ^ t.Ast.tname ^ "(" ^ String.concat ", " params ^ ")"
          in
          let parameters =
            List.map
              (fun s -> ParameterInformation.create ~label:(`String s) ())
              params
          in
          let commas =
            String.fold_left
              (fun n c -> if c = ',' then n + 1 else n)
              0
              (String.sub text t.Ast.tspan.Span.start.offset
                 (off - t.Ast.tspan.Span.start.offset))
          in
          let signature = SignatureInformation.create ~label ~parameters () in
          Some
            (SignatureHelp.create ~signatures:[ signature ] ~activeSignature:0
               ~activeParameter:(Some (min commas (List.length params - 1)))
               ()))
