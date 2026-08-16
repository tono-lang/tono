(* Extension-model validation at the AST level, where source spans still exist
   (the IR carries none). The generator owns the conformance gate (a contract
   with no conformance reference refuses to emit); this pass owns the structural
   rules the IR alone cannot express:

   - a hook is rejected outright: the lifecycle it used to fill is gone,
     replaced by the declarative `ext <lib> { extern ... }` FFI model, and the
     diagnostic names the substitute for the slot it tried to fill (TC0027);
   - a contract requires a typed signature, and an impl takes the signature of
     the operation it names and declares none (TC0029);
   - the raw response form is an impl affordance: on any other kind it would be
     accepted and then silently ignored (TC0052);
   - a binding must target a supported language (TC0028);
   - an extension must carry at least one binding, or it is inert (TC0030);
   - a binding value must be a "file#symbol" reference so the generator can
     resolve the bound symbol (TC0031);
   - two extensions must not share a name (TC0032);
   - a language must be bound at most once per extension: two bindings for one
     language lower into a JSON object with a duplicate key, and the Rust serde
     mirror (a map) keeps only the last, silently dropping a binding and breaking
     the cross-language round-trip (TC0033). *)

let err code span fmt = Printf.ksprintf (Diagnostic.error ~code span) fmt

(* The substitute named in the migration diagnostic for each of the four
   lifecycle slots the hook mechanism used to fill (the ext/extern FFI model replaced the
   hooks). A name outside this table gets the generic message below: nothing
   in a hook's name can be valid anymore, so unlike the closed-slot check this
   replaced, this list only drives which substitute to name. *)
let hook_slot_substitutes =
  [
    ( "client_init",
      "read external config through a field with an 'extern' call, or build a \
       header from a field via @format + @header" );
    ( "before_request",
      "bind an external value to a trait argument, e.g. \
       @header(\"Authorization\", mylib.sign(.request))" );
    ("after_response", "a declared projection in the op's 'extern returns:'");
    ("on_error", "'errors:' in the ext language block plus @errors on the op");
  ]

let hook_removed_message (slot : string) : string =
  match List.assoc_opt slot hook_slot_substitutes with
  | Some substitute ->
      Printf.sprintf
        "the '%s' hook is removed; the lifecycle it filled no longer exists. \
         Use %s"
        slot substitute
  | None ->
      Printf.sprintf
        "hooks are removed; libraries are integrated via 'ext <lib> { extern \
         ... }', not lifecycle slots"

(* The languages a binding may target: the five backends plus the two short
   aliases the toolchain accepts. *)
let valid_languages =
  [ "ts"; "typescript"; "rust"; "go"; "py"; "python"; "java" ]

(* [raw] switches the impl to the outcome-decoding path; on a hook, contract, or
   constraint there is no response to decode, so the word would be inert. *)
let check_raw ekind eraw : Diagnostic.t list =
  match (ekind, eraw) with
  | Ast.EImpl, _ | _, None -> []
  | _, Some span ->
      [
        err Error_codes.ext_raw_rule span
          "'raw' is the impl response form and applies to no other extension \
           kind";
      ]

let check_kind (d : Ast.decl) ekind ekind_span esig : Diagnostic.t list =
  match ekind with
  | Ast.EHook ->
      [
        err Error_codes.ext_hook_removed d.dname_span "%s"
          (hook_removed_message d.Ast.dname);
      ]
  | Ast.EContract ->
      if esig = None then
        [
          err Error_codes.ext_signature_rule ekind_span
            "a contract requires a typed '(input) -> output' signature";
        ]
      else []
  | Ast.EConstraint -> []
  | Ast.EImpl ->
      if esig <> None then
        [
          err Error_codes.ext_signature_rule ekind_span
            "an impl takes the signature of the operation it names and \
             declares none";
        ]
      else []

(* A binding value must be "ext/{lang}/path#symbol": a single '#' splitting a
   non-empty file path from a non-empty symbol, so the generator can import the
   bound symbol. A missing '#' would otherwise drop the binding silently. *)
let binding_reference_ok (target : string) : bool =
  match String.index_opt target '#' with
  | Some i -> i > 0 && i < String.length target - 1
  | None -> false

let check_bindings (d : Ast.decl) (bindings : Ast.ext_binding list) :
    Diagnostic.t list =
  let lang_diags =
    List.filter_map
      (fun (b : Ast.ext_binding) ->
        if List.mem b.Ast.lang valid_languages then None
        else
          Some
            (err Error_codes.ext_binding_language_invalid b.lang_span
               "unsupported binding language '%s'" b.lang))
      bindings
  in
  let format_diags =
    List.filter_map
      (fun (b : Ast.ext_binding) ->
        if binding_reference_ok b.Ast.target then None
        else
          Some
            (err Error_codes.ext_binding_malformed b.lang_span
               "binding '%s' must be a \"file#symbol\" reference" b.target))
      bindings
  in
  let missing =
    if bindings = [] then
      [
        err Error_codes.ext_binding_missing d.dname_span
          "extension '%s' has no binding; bind it to at least one language"
          d.dname;
      ]
    else []
  in
  (* A language bound twice collapses to one wire key (last wins) once the assoc
     list encodes to a JSON object, so a second binding for the same language is
     rejected here at its own span. *)
  let seen = Hashtbl.create 4 in
  let duplicate_diags =
    List.filter_map
      (fun (b : Ast.ext_binding) ->
        if Hashtbl.mem seen b.Ast.lang then
          Some
            (err Error_codes.ext_binding_duplicate_language b.lang_span
               "language '%s' is already bound; bind each language at most once"
               b.lang)
        else (
          Hashtbl.add seen b.Ast.lang ();
          None))
      bindings
  in
  lang_diags @ format_diags @ missing @ duplicate_diags

let check_decl (d : Ast.decl) : Diagnostic.t list =
  match d.dkind with
  | Ast.DExt { ekind; ekind_span; esig; eraw; ebindings; _ } ->
      check_kind d ekind ekind_span esig
      @ check_raw ekind eraw @ check_bindings d ebindings
  | Ast.DStruct _ | Ast.DEnum _ | Ast.DUnion _ | Ast.DOp _ | Ast.DExtLib _
  | Ast.DTest _ ->
      []

(* Extensions share one namespace (a hook's name is its slot), and they never
   enter the shape symbol table, so a repeated name is diagnosed here rather than
   as a duplicate shape. Without this the second binding would silently shadow the
   first (hook lookup keeps the first match). *)
let check_duplicates (decls : Ast.decl list) : Diagnostic.t list =
  let seen = Hashtbl.create 8 in
  List.filter_map
    (fun (d : Ast.decl) ->
      match d.dkind with
      | Ast.DExt _ ->
          if Hashtbl.mem seen d.dname then
            Some
              (err Error_codes.ext_duplicate d.dname_span
                 "duplicate extension '%s'" d.dname)
          else (
            Hashtbl.add seen d.dname ();
            None)
      | _ -> None)
    decls

let check_decls (decls : Ast.decl list) : Diagnostic.t list =
  List.concat_map check_decl decls @ check_duplicates decls
