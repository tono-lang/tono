(* Internal-consistency typecheck for the "ext"/"extern" FFI library block
   (RFC-0023). Two independent concerns:

   - Per extern, per language binding: a call: arg naming an undeclared
     logical parameter (TC0070); a ctor literal projected into a declared
     foreign struct disagreeing in field name or type with the parameter it
     forwards (TC0071); a yields: position nothing in returns:/errors:
     consumes (TC0072); more than one "error"-typed yields: position
     (TC0073); a returns: with no yields: to project from (TC0074); a
     returns: that builds a type other than the extern's own declared
     logical return (TC0075); a returns: field ref whose head is not a
     declared yields: name (TC0076); an errors: sentinel mapped to a type
     that does not resolve (TC0077); a logical parameter this language's
     call: never consumes (TC0078).

   - Cross-file closed accounting (decision K, [check_project]): the same
     "ext" name's module path for one language declared with two different
     targets (TC0079); an extern (or opaque-type method) name repeated
     within one "ext", even across files (TC0080); a language block for a
     target the "ext" declares no module path for (TC0081).

   Never verifies that a declared foreign symbol really exists in the target
   library: that is the target compiler's job (RFC's degrau 2), out of scope
   here. The foreign-role wire/surface boundary (RFC's "papel novo") lives in
   [Roles]/[Check_entries] instead, since it reuses their existing closed-
   boundary machinery. *)

let err code span fmt = Printf.ksprintf (Diagnostic.error ~code span) fmt

(* ── Parameter reference collection ────────────────────────────────────── *)

(* A call: arg or ctor field value that names a logical parameter, walked
   recursively through nested ctors and nested "ns.fn(...)" calls (their
   arguments can reference this extern's own parameters too). *)
let rec collect_call_arg : Ast.call_arg -> string list = function
  | Ast.CaParam (n, _) -> [ n ]
  | Ast.CaRef _ -> []
  | Ast.CaCtor c ->
      List.concat_map (fun (_, _, v) -> collect_trait_arg v) c.Ast.ctor_fields

and collect_trait_arg : Ast.trait_arg -> string list = function
  | Ast.AName n -> [ n ]
  | Ast.AKv (_, v) -> collect_trait_arg v
  | Ast.AList xs -> List.concat_map collect_trait_arg xs
  | Ast.ACtor c ->
      List.concat_map (fun (_, _, v) -> collect_trait_arg v) c.Ast.ctor_fields
  | Ast.ACall ce -> List.concat_map collect_call_arg ce.Ast.ce_args
  | Ast.AString _ | Ast.AInt _ | Ast.AFloat _ | Ast.ARef _ -> []

(* Every unknown-parameter diagnostic, walked the same way, anchored at the
   best span available (the arg's own span, or its ctor field's key span for
   a nested [Ast.trait_arg] which carries none of its own). *)
let rec unknown_param_call_arg (declared : string list) :
    Ast.call_arg -> Diagnostic.t list = function
  | Ast.CaParam (n, span) ->
      if List.mem n declared then []
      else
        [
          err Error_codes.extern_call_unknown_param span
            "'%s' is not a declared logical parameter of this extern" n;
        ]
  | Ast.CaRef _ -> []
  | Ast.CaCtor c ->
      List.concat_map
        (fun (_, span, v) -> unknown_param_trait_arg declared span v)
        c.Ast.ctor_fields

and unknown_param_trait_arg (declared : string list) (span : Span.span) :
    Ast.trait_arg -> Diagnostic.t list = function
  | Ast.AName n ->
      if List.mem n declared then []
      else
        [
          err Error_codes.extern_call_unknown_param span
            "'%s' is not a declared logical parameter of this extern" n;
        ]
  | Ast.AKv (_, v) -> unknown_param_trait_arg declared span v
  | Ast.AList xs -> List.concat_map (unknown_param_trait_arg declared span) xs
  | Ast.ACtor c ->
      List.concat_map
        (fun (_, fspan, v) -> unknown_param_trait_arg declared fspan v)
        c.Ast.ctor_fields
  | Ast.ACall ce ->
      List.concat_map (unknown_param_call_arg declared) ce.Ast.ce_args
  | Ast.AString _ | Ast.AInt _ | Ast.AFloat _ | Ast.ARef _ -> []

(* ── One extern's per-language rules ───────────────────────────────────── *)

(* A call: ctor projecting into a declared foreign struct: every field must
   exist on that struct, and a field forwarding a logical parameter must
   agree with the struct field's declared type (the tractable slice of "call
   types match the signature" -- full expression-level inference for the
   call sublanguage exists nowhere else in the codebase). *)
let check_ctor_projection (structs : (string, Ast.foreign_struct) Hashtbl.t)
    (params : Ast.extern_param list) (c : Ast.ctor_arg) : Diagnostic.t list =
  match Hashtbl.find_opt structs c.Ast.ctor_name with
  | None ->
      [
        err Error_codes.extern_call_type_mismatch c.Ast.ctor_name_span
          "'%s' is not a foreign struct declared in this ext block"
          c.Ast.ctor_name;
      ]
  | Some s ->
      List.concat_map
        (fun (fname, fspan, v) ->
          match
            List.find_opt
              (fun (ff : Ast.foreign_field) ->
                String.equal ff.Ast.ff_name fname)
              s.Ast.fs_fields
          with
          | None ->
              [
                err Error_codes.extern_call_type_mismatch fspan
                  "'%s' is not a field of foreign struct '%s'" fname
                  s.Ast.fs_name;
              ]
          | Some ff -> (
              match v with
              | Ast.AName pname -> (
                  match
                    List.find_opt
                      (fun (p : Ast.extern_param) ->
                        String.equal p.Ast.ep_name pname)
                      params
                  with
                  | Some p
                    when not
                           (String.equal
                              (Printer.print_ty p.Ast.ep_type)
                              (Printer.print_ty ff.Ast.ff_type)) ->
                      [
                        err Error_codes.extern_call_type_mismatch fspan
                          "parameter '%s' is %s but foreign field '%s.%s' is %s"
                          pname
                          (Printer.print_ty p.Ast.ep_type)
                          s.Ast.fs_name fname
                          (Printer.print_ty ff.Ast.ff_type);
                      ]
                  | _ -> [])
              | _ -> []))
        c.Ast.ctor_fields

let consumed_heads (r : Ast.returns_lit option) : string list =
  match r with
  | None -> []
  | Some rl ->
      List.filter_map
        (fun (f : Ast.returns_field) ->
          match f.Ast.rf_value with
          | Ast.RvRef rp -> (
              match rp.Ast.segs with h :: _ -> Some h | [] -> None)
          | Ast.RvMatch fm -> (
              match fm.Ast.subject.Ast.segs with h :: _ -> Some h | [] -> None))
        rl.Ast.rl_fields

(* Every yields: position must be consumed: a plain position by a returns:
   ref reading it, the one reserved "error" position by the mere presence of
   an errors: mapping (nothing else could reference it -- error is not a
   type, see [Ast.yields_ty]). *)
let check_yields_consumption (b : Ast.extern_lang_body) : Diagnostic.t list =
  match b.Ast.elb_yields with
  | None -> []
  | Some ys ->
      let consumed = consumed_heads b.Ast.elb_returns in
      let has_errors = b.Ast.elb_errors <> [] in
      List.concat_map
        (fun (y : Ast.yields_pos) ->
          match y.Ast.yp_ty with
          | Ast.YError _ ->
              if has_errors then []
              else
                [
                  err Error_codes.extern_yields_position_dead y.Ast.yp_name_span
                    "yields position '%s' is the reserved error position but \
                     no 'errors:' mapping consumes it"
                    y.Ast.yp_name;
                ]
          | Ast.YType _ ->
              if List.mem y.Ast.yp_name consumed then []
              else
                [
                  err Error_codes.extern_yields_position_dead y.Ast.yp_name_span
                    "yields position '%s' is never consumed by 'returns:' or \
                     'errors:'"
                    y.Ast.yp_name;
                ])
        ys

(* At most one yields: position may be the reserved "error" type. *)
let check_single_error_position (ys : Ast.yields_pos list) : Diagnostic.t list =
  let errs =
    List.filter
      (fun (y : Ast.yields_pos) ->
        match y.Ast.yp_ty with Ast.YError _ -> true | Ast.YType _ -> false)
      ys
  in
  match errs with
  | [] | [ _ ] -> []
  | _ :: rest ->
      List.map
        (fun (y : Ast.yields_pos) ->
          err Error_codes.extern_yields_multiple_errors y.Ast.yp_name_span
            "at most one 'yields:' position may be the reserved 'error' type; \
             '%s' is a second"
            y.Ast.yp_name)
        rest

(* returns: is the only case yields: may be omitted from; a projection with
   nothing declared to project from is meaningless. *)
let check_returns_requires_yields (b : Ast.extern_lang_body) : Diagnostic.t list
    =
  match (b.Ast.elb_returns, b.Ast.elb_yields) with
  | Some rl, None ->
      [
        err Error_codes.extern_yields_required rl.Ast.rl_span
          "'returns:' projects a value but this binding declares no 'yields:' \
           to project from";
      ]
  | _ -> []

(* returns: builds the extern's own declared logical type, never another
   one: "Tipo { ... }" means the same thing here as everywhere else in tono. *)
let check_returns_type (ed_return : Ast.ty) (b : Ast.extern_lang_body) :
    Diagnostic.t list =
  match b.Ast.elb_returns with
  | None -> []
  | Some rl ->
      if
        String.equal
          (Printer.print_ty rl.Ast.rl_type)
          (Printer.print_ty ed_return)
      then []
      else
        [
          err Error_codes.extern_returns_type_mismatch rl.Ast.rl_span
            "'returns:' builds '%s', but the extern's declared logical return \
             is '%s'"
            (Printer.print_ty rl.Ast.rl_type)
            (Printer.print_ty ed_return);
        ]

(* Every returns: field ref's head segment must name a declared yields:
   position; that is how the projection's origin is reached. *)
let check_returns_refs (b : Ast.extern_lang_body) : Diagnostic.t list =
  match b.Ast.elb_returns with
  | None -> []
  | Some rl ->
      let declared =
        match b.Ast.elb_yields with
        | None -> []
        | Some ys -> List.map (fun (y : Ast.yields_pos) -> y.Ast.yp_name) ys
      in
      List.concat_map
        (fun (f : Ast.returns_field) ->
          let segs, span =
            match f.Ast.rf_value with
            | Ast.RvRef rp -> (rp.Ast.segs, rp.Ast.ref_span)
            | Ast.RvMatch fm ->
                (fm.Ast.subject.Ast.segs, fm.Ast.subject.Ast.ref_span)
          in
          match segs with
          | h :: _ when List.mem h declared -> []
          | h :: _ ->
              [
                err Error_codes.extern_returns_ref_unknown span
                  "'.%s' does not resolve into a declared 'yields:' position" h;
              ]
          | [] -> [])
        rl.Ast.rl_fields

(* An errors: sentinel must map to a type declared somewhere in this module
   (the wire error the fronter, generated glue, and testing surface all
   need). *)
let check_error_sentinels ~(tbl : Symtab.t) (b : Ast.extern_lang_body) :
    Diagnostic.t list =
  List.filter_map
    (fun (e : Ast.error_map_entry) ->
      match Symtab.find e.Ast.em_type tbl with
      | Some _ -> None
      | None ->
          Some
            (err Error_codes.extern_error_sentinel_unknown e.Ast.em_type_span
               "unknown error type '%s'; declare it before mapping a sentinel \
                to it"
               e.Ast.em_type))
    b.Ast.elb_errors

(* Every logical parameter must be consumed by this language's own call:,
   recursively through any ctor projection or nested call. *)
let check_param_consumption (params : Ast.extern_param list)
    (b : Ast.extern_lang_body) : Diagnostic.t list =
  let referenced = List.concat_map collect_call_arg b.Ast.elb_call_args in
  List.filter_map
    (fun (p : Ast.extern_param) ->
      if List.mem p.Ast.ep_name referenced then None
      else
        Some
          (err Error_codes.extern_param_unconsumed p.Ast.ep_name_span
             "logical parameter '%s' is never consumed by the '%s' binding's \
              call"
             p.Ast.ep_name b.Ast.elb_lang))
    params

let check_extern ~(tbl : Symtab.t)
    (structs : (string, Ast.foreign_struct) Hashtbl.t) (e : Ast.extern_decl) :
    Diagnostic.t list =
  let declared_param_names =
    List.map (fun (p : Ast.extern_param) -> p.Ast.ep_name) e.Ast.ed_params
  in
  List.concat_map
    (fun (b : Ast.extern_lang_body) ->
      List.concat_map
        (unknown_param_call_arg declared_param_names)
        b.Ast.elb_call_args
      @ List.concat_map
          (function
            | Ast.CaCtor c -> check_ctor_projection structs e.Ast.ed_params c
            | Ast.CaParam _ | Ast.CaRef _ -> [])
          b.Ast.elb_call_args
      @ check_yields_consumption b
      @ (match b.Ast.elb_yields with
        | Some ys -> check_single_error_position ys
        | None -> [])
      @ check_returns_requires_yields b
      @ check_returns_type e.Ast.ed_return b
      @ check_returns_refs b
      @ check_error_sentinels ~tbl b
      @ check_param_consumption e.Ast.ed_params b)
    e.Ast.ed_langs

(* ── Foreign-name collisions (not a silent Roles.classify overwrite) ────── *)

let check_foreign_name_collisions ~(tbl : Symtab.t) (decls : Ast.decl list) :
    Diagnostic.t list =
  let seen = Hashtbl.create 8 in
  let check_name name span =
    let shape_diag =
      match Symtab.find name tbl with
      | Some _ ->
          [
            err Error_codes.extern_duplicate_name span
              "'%s' collides with a declared shape name" name;
          ]
      | None -> []
    in
    let dup_diag =
      if Hashtbl.mem seen name then
        [
          err Error_codes.extern_duplicate_name span
            "'%s' is declared more than once across this module's 'ext' \
             library blocks"
            name;
        ]
      else (
        Hashtbl.add seen name ();
        [])
    in
    shape_diag @ dup_diag
  in
  List.concat_map
    (fun (d : Ast.decl) ->
      match d.Ast.dkind with
      | Ast.DExtLib { body; _ } ->
          List.concat_map
            (fun (s : Ast.foreign_struct) ->
              check_name s.Ast.fs_name s.Ast.fs_name_span)
            body.Ast.elib_structs
          @ List.concat_map
              (fun (t : Ast.opaque_type) ->
                check_name t.Ast.opq_name t.Ast.opq_name_span)
              body.Ast.elib_types
      | _ -> [])
    decls

(* ── "ext" blocks as a namespace for Resolve ─────────────────────────────── *)

let qualified_of (decls : Ast.decl list) (fallback : Resolve.qualified) :
    Resolve.qualified =
 fun ~qualifier ~name ~n_args span ->
  let names_opaque_type =
    List.exists
      (fun (d : Ast.decl) ->
        String.equal d.Ast.dname qualifier
        &&
        match d.Ast.dkind with
        | Ast.DExtLib { body; _ } ->
            List.exists
              (fun (t : Ast.opaque_type) -> String.equal t.Ast.opq_name name)
              body.Ast.elib_types
        | _ -> false)
      decls
  in
  if names_opaque_type then
    if n_args = 0 then []
    else
      [
        err Error_codes.non_generic_applied span
          "'%s.%s' is not generic and takes no type arguments" qualifier name;
      ]
  else fallback ~qualifier ~name ~n_args span

(* ── Per-module entry point ────────────────────────────────────────────── *)

let check_decls ~(tbl : Symtab.t) (decls : Ast.decl list) : Diagnostic.t list =
  List.concat_map
    (fun (d : Ast.decl) ->
      match d.Ast.dkind with
      | Ast.DExtLib { body; _ } ->
          let structs = Hashtbl.create 8 in
          List.iter
            (fun (s : Ast.foreign_struct) ->
              Hashtbl.replace structs s.Ast.fs_name s)
            body.Ast.elib_structs;
          let free_diags =
            List.concat_map (check_extern ~tbl structs) body.Ast.elib_externs
          in
          let method_diags =
            List.concat_map
              (fun (t : Ast.opaque_type) ->
                List.concat_map (check_extern ~tbl structs) t.Ast.opq_methods)
              body.Ast.elib_types
          in
          free_diags @ method_diags
      | _ -> [])
    decls
  @ check_foreign_name_collisions ~tbl decls

(* ── Cross-file closed accounting (decision K) ─────────────────────────── *)

type occurrence = { file : string; body : Ast.ext_lib_body }

(* Every ext-block occurrence, grouped by its declared name, across every
   named decl-list (one entry per file, or a single "" entry for a lone
   module). *)
let occurrences_by_name (files : (string * Ast.decl list) list) :
    (string, occurrence list) Hashtbl.t =
  let groups = Hashtbl.create 8 in
  List.iter
    (fun (file, decls) ->
      List.iter
        (fun (d : Ast.decl) ->
          match d.Ast.dkind with
          | Ast.DExtLib { body; _ } ->
              let prev =
                Option.value ~default:[] (Hashtbl.find_opt groups d.Ast.dname)
              in
              Hashtbl.replace groups d.Ast.dname ({ file; body } :: prev)
          | _ -> ())
        decls)
    files;
  groups

(* A language's module path declared with two different targets across the
   group is a conflict; every declaring occurrence is flagged, each labeled
   with its own file. A repeated declaration of the *same* path is not an
   error (the RFC only calls out a conflicting one). *)
let check_module_paths (ext_name : string) (occs : occurrence list) :
    (string * Diagnostic.t) list =
  let entries =
    List.concat_map
      (fun occ ->
        List.map
          (fun (lp : Ast.lang_path) ->
            (lp.Ast.lp_lang, lp.Ast.lp_path, lp.Ast.lp_lang_span, occ.file))
          occ.body.Ast.elib_langs)
      occs
  in
  let langs =
    List.sort_uniq compare (List.map (fun (l, _, _, _) -> l) entries)
  in
  List.concat_map
    (fun lang ->
      let group =
        List.filter (fun (l, _, _, _) -> String.equal l lang) entries
      in
      let paths =
        List.sort_uniq compare (List.map (fun (_, p, _, _) -> p) group)
      in
      if List.length paths <= 1 then []
      else
        List.map
          (fun (_, path, span, file) ->
            ( file,
              err Error_codes.ext_lib_module_path_conflict span
                "module path for '%s' in ext '%s' is declared as '%s' here, \
                 but differently elsewhere"
                lang ext_name path ))
          group)
    langs

(* A name repeated within one namespace (the ext's free externs, or one
   opaque type's own methods) is a conflict, even across files; the first
   declaration wins silently, every later one is flagged. *)
let dup_name_diags ~code (items : (string * Span.span * string) list) :
    (string * Diagnostic.t) list =
  let seen = Hashtbl.create 8 in
  List.filter_map
    (fun (name, span, file) ->
      if Hashtbl.mem seen name then
        Some (file, err code span "'%s' is already declared" name)
      else (
        Hashtbl.add seen name ();
        None))
    items

let check_duplicate_names (occs : occurrence list) :
    (string * Diagnostic.t) list =
  let frees =
    List.concat_map
      (fun occ ->
        List.map
          (fun (e : Ast.extern_decl) ->
            (e.Ast.ed_name, e.Ast.ed_name_span, occ.file))
          occ.body.Ast.elib_externs)
      occs
  in
  let methods_by_type = Hashtbl.create 8 in
  List.iter
    (fun occ ->
      List.iter
        (fun (t : Ast.opaque_type) ->
          let prev =
            Option.value ~default:[]
              (Hashtbl.find_opt methods_by_type t.Ast.opq_name)
          in
          Hashtbl.replace methods_by_type t.Ast.opq_name
            (prev
            @ List.map
                (fun (m : Ast.extern_decl) ->
                  (m.Ast.ed_name, m.Ast.ed_name_span, occ.file))
                t.Ast.opq_methods))
        occ.body.Ast.elib_types)
    occs;
  dup_name_diags ~code:Error_codes.extern_duplicate_name frees
  @ List.concat_map
      (fun items ->
        dup_name_diags ~code:Error_codes.extern_duplicate_name items)
      (Hashtbl.fold (fun _ items acc -> items :: acc) methods_by_type [])

(* A language block bound in a call: for a target the ext declares no module
   path for is inert: nothing tells the generator where the symbol lives. *)
let check_lang_has_module (ext_name : string) (occs : occurrence list) :
    (string * Diagnostic.t) list =
  let declared_langs =
    List.sort_uniq compare
      (List.concat_map
         (fun occ ->
           List.map
             (fun (lp : Ast.lang_path) -> lp.Ast.lp_lang)
             occ.body.Ast.elib_langs)
         occs)
  in
  let externs_of occ =
    occ.body.Ast.elib_externs
    @ List.concat_map
        (fun (t : Ast.opaque_type) -> t.Ast.opq_methods)
        occ.body.Ast.elib_types
  in
  List.concat_map
    (fun occ ->
      List.concat_map
        (fun (e : Ast.extern_decl) ->
          List.filter_map
            (fun (b : Ast.extern_lang_body) ->
              if List.mem b.Ast.elb_lang declared_langs then None
              else
                Some
                  ( occ.file,
                    err Error_codes.extern_lang_no_module b.Ast.elb_lang_span
                      "language '%s' has no declared module path in ext '%s'"
                      b.Ast.elb_lang ext_name ))
            e.Ast.ed_langs)
        (externs_of occ))
    occs

let check_project (files : (string * Ast.decl list) list) : Diagnostic.t list =
  let groups = occurrences_by_name files in
  Hashtbl.fold
    (fun ext_name occs acc ->
      let attributed =
        check_module_paths ext_name occs
        @ check_duplicate_names occs
        @ check_lang_has_module ext_name occs
      in
      List.map
        (fun (file, d) ->
          if String.equal file "" then d
          else
            { d with Diagnostic.message = file ^ ": " ^ d.Diagnostic.message })
        attributed
      @ acc)
    groups []
