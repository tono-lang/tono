(* Internal-consistency typecheck for the "ext"/"extern" FFI library block,
   per extern, per language binding: a call: arg naming an undeclared logical
   parameter (TC0070); a ctor literal projected into a declared foreign
   struct disagreeing in field name or type with the parameter it forwards
   (TC0071); a yields: position nothing in returns:/errors: consumes
   (TC0072); more than one "error"-typed yields: position (TC0073); a
   returns: with no yields: to project from (TC0074); a returns: that builds
   a type other than the extern's own declared logical return (TC0075); a
   returns: field ref whose head is not a declared yields: name (TC0076); an
   errors: sentinel mapped to a type that does not resolve (TC0077); a
   logical parameter this language's call: never consumes (TC0078).

   The cross-file closed accounting (decision K: TC0079-TC0081) lives in
   [Check_ext_lib_project], split out to keep this file under the line-count
   cap.

   Never verifies that a declared foreign symbol really exists in the target
   library: that is the target compiler's own job, out of scope here. The
   foreign-role wire/surface boundary lives in [Roles]/[Check_entries]
   instead, since it reuses their existing closed-boundary machinery. *)

let err code span fmt = Printf.ksprintf (Diagnostic.error ~code span) fmt

(* ── Parameter reference collection ────────────────────────────────────── *)

(* A call: arg or ctor field value that names a logical parameter, walked
   recursively through nested ctors and nested "ns.fn(...)" calls (their
   arguments can reference this extern's own parameters too). *)
let rec collect_call_arg : Ast.call_arg -> string list = function
  | Ast.CaParam (n, _) -> [ n ]
  | Ast.CaRef _ | Ast.CaLit _ | Ast.CaType _ -> []
  | Ast.CaCtor c ->
      List.concat_map (fun (_, _, v) -> collect_trait_arg v) c.Ast.ctor_fields
  | Ast.CaCall nc -> List.concat_map collect_call_arg nc.Ast.nc_args
  | Ast.CaList (items, _) -> List.concat_map collect_call_arg items

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
  | Ast.CaRef _ | Ast.CaLit _ | Ast.CaType _ -> []
  | Ast.CaCtor c ->
      List.concat_map
        (fun (_, span, v) -> unknown_param_trait_arg declared span v)
        c.Ast.ctor_fields
  | Ast.CaCall nc ->
      List.concat_map (unknown_param_call_arg declared) nc.Ast.nc_args
  | Ast.CaList (items, _) ->
      List.concat_map (unknown_param_call_arg declared) items

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

(* [check_ctor_projection], walked over a whole [call_arg], including into a
   nested call's own arguments (a [CaCall] can carry a ctor argument too,
   e.g. [WithPrecision(opts { retries: 3 })]). *)
let rec check_ctor_projection_arg
    (structs : (string, Ast.foreign_struct) Hashtbl.t)
    (params : Ast.extern_param list) : Ast.call_arg -> Diagnostic.t list =
  function
  | Ast.CaCtor c -> check_ctor_projection structs params c
  | Ast.CaCall nc ->
      List.concat_map (check_ctor_projection_arg structs params) nc.Ast.nc_args
  | Ast.CaList (items, _) ->
      List.concat_map (check_ctor_projection_arg structs params) items
  | Ast.CaParam _ | Ast.CaRef _ | Ast.CaLit _ | Ast.CaType _ -> []

(* A class reference ([type name]) must name an opaque handle declared in
   this same ext block (TC0098): that declaration is what carries the
   foreign name the emitter passes, per language. Walked through the same
   nesting a nested call or list allows. *)
let rec check_type_args (handles : string list) :
    Ast.call_arg -> Diagnostic.t list = function
  | Ast.CaType (n, span) ->
      if List.mem n handles then []
      else
        [
          err Error_codes.extern_type_arg_unknown span
            "'%s' is not an opaque handle declared in this ext block; a class \
             reference names the handle whose foreign type the library \
             constructs"
            n;
        ]
  | Ast.CaCall nc -> List.concat_map (check_type_args handles) nc.Ast.nc_args
  | Ast.CaList (items, _) -> List.concat_map (check_type_args handles) items
  | Ast.CaParam _ | Ast.CaRef _ | Ast.CaLit _ | Ast.CaCtor _ -> []

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

(* The [ctx] marker only has an idiomatic slot to fill on a foreign handle's
   own method call (Go's generated op signatures already carry [ctx
   context.Context] in scope there); a free/library extern (including a
   field's own construction call) has no such call site, so marking it
   there is rejected rather than silently ignored. *)
let check_ctx_marker_scope ~(is_method : bool) (b : Ast.extern_lang_body) :
    Diagnostic.t list =
  if b.Ast.elb_ctx && not is_method then
    [
      err Error_codes.extern_ctx_on_free_call b.Ast.elb_span
        "'ctx' only applies to a foreign handle's own method call; the '%s' \
         binding here has no idiomatic scope to receive it"
        b.Ast.elb_lang;
    ]
  else []

(* A type receiver ("Type"."method") only makes sense on a free extern: a
   handle's own method already has its receiver (the handle), and "new"
   constructs a type rather than calling a method on it. *)
let check_receiver_scope ~(is_method : bool) (b : Ast.extern_lang_body) :
    Diagnostic.t list =
  match (b.Ast.elb_call_receiver, b.Ast.elb_call_receiver_span) with
  | Some r, Some span ->
      (if is_method then
         [
           err Error_codes.extern_receiver_on_method span
             "'%s' names a type as the receiver of a foreign handle's own \
              method; the handle is already the receiver of the '%s' call"
             r b.Ast.elb_lang;
         ]
       else [])
      @
      if b.Ast.elb_new then
        [
          err Error_codes.extern_receiver_with_new span
            "'new' constructs a type and a type receiver calls a static method \
             on it; the '%s' binding cannot do both"
            b.Ast.elb_lang;
        ]
      else []
  | _ -> []

let check_extern ~(tbl : Symtab.t)
    (structs : (string, Ast.foreign_struct) Hashtbl.t) ~(handles : string list)
    ~(is_method : bool) (e : Ast.extern_decl) : Diagnostic.t list =
  let declared_param_names =
    List.map (fun (p : Ast.extern_param) -> p.Ast.ep_name) e.Ast.ed_params
  in
  List.concat_map
    (fun (b : Ast.extern_lang_body) ->
      List.concat_map
        (unknown_param_call_arg declared_param_names)
        b.Ast.elb_call_args
      @ List.concat_map
          (fun a -> check_ctor_projection_arg structs e.Ast.ed_params a)
          b.Ast.elb_call_args
      @ List.concat_map (check_type_args handles) b.Ast.elb_call_args
      @ check_yields_consumption b
      @ (match b.Ast.elb_yields with
        | Some ys -> check_single_error_position ys
        | None -> [])
      @ check_returns_requires_yields b
      @ check_returns_type e.Ast.ed_return b
      @ check_returns_refs b
      @ check_error_sentinels ~tbl b
      @ check_param_consumption e.Ast.ed_params b
      @ check_ctx_marker_scope ~is_method b
      @ check_receiver_scope ~is_method b)
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

let is_foreign_name (decls : Ast.decl list) (name : string) : bool =
  List.exists
    (fun (d : Ast.decl) ->
      match d.Ast.dkind with
      | Ast.DExtLib { body; _ } ->
          List.exists
            (fun (s : Ast.foreign_struct) -> String.equal s.Ast.fs_name name)
            body.Ast.elib_structs
          || List.exists
               (fun (t : Ast.opaque_type) -> String.equal t.Ast.opq_name name)
               body.Ast.elib_types
      | _ -> false)
    decls

(* ── Instantiation clauses ("type Name(\"Foreign\", Arg) { ... }") ──────── *)

(* The argument must be a tono type already declared in this module: never a
   foreign name (a foreign struct/opaque type is not "known" here, so it
   falls through to an ordinary unresolved-name diagnostic, TC0001), and
   never a type parameter (there is none to be in scope at module level). *)
let check_instance_arg ~(tbl : Symtab.t) (t : Ast.opaque_type) :
    Diagnostic.t list =
  match t.Ast.opq_instance with
  | None -> []
  | Some i ->
      Resolve.resolve_ty ~params:[] ~tbl ~qualified:Resolve.no_imports
        i.Ast.oi_arg

(* A keyed instantiation name list must be exactly one entry per language
   the "ext" declares a module path for: a language named twice, a language
   with no declared module path (same spirit as TC0081), or a declared
   language left without a name are all diagnosed, so every target's emitter
   has exactly one spelling to read. The shared (single-string) form is
   exempt: it covers every language by construction. *)
let check_instance_names (body : Ast.ext_lib_body) (t : Ast.opaque_type) :
    Diagnostic.t list =
  match t.Ast.opq_instance with
  | None | Some { oi_names = Ast.OnShared _; _ } -> []
  | Some ({ oi_names = Ast.OnPerLang entries; _ } as i) ->
      let declared =
        List.map (fun (lp : Ast.lang_path) -> lp.Ast.lp_lang) body.elib_langs
      in
      let dups =
        List.filteri
          (fun idx (e : Ast.opaque_name_entry) ->
            List.exists
              (fun (prev : Ast.opaque_name_entry) ->
                String.equal prev.one_lang e.one_lang)
              (List.filteri (fun j _ -> j < idx) entries))
          entries
      in
      let dup_diags =
        List.map
          (fun (e : Ast.opaque_name_entry) ->
            err Error_codes.instance_names_mismatch e.one_lang_span
              "the instantiation already names its '%s' type" e.one_lang)
          dups
      in
      let unknown =
        List.filter
          (fun (e : Ast.opaque_name_entry) ->
            not (List.mem e.one_lang declared))
          entries
      in
      let unknown_diags =
        List.map
          (fun (e : Ast.opaque_name_entry) ->
            err Error_codes.instance_names_mismatch e.one_lang_span
              "the ext declares no '%s' module path for this instantiation \
               name to bind to"
              e.one_lang)
          unknown
      in
      let named lang =
        List.exists
          (fun (e : Ast.opaque_name_entry) -> String.equal e.one_lang lang)
          entries
      in
      let missing = List.filter (fun lang -> not (named lang)) declared in
      let missing_diags =
        List.map
          (fun lang ->
            err Error_codes.instance_names_mismatch i.Ast.oi_span
              "the instantiation names no '%s' type, but the ext declares a \
               '%s' module path"
              lang lang)
          missing
      in
      dup_diags @ unknown_diags @ missing_diags

(* The names an instantiation claims, one (lang, name) pair per language: the
   shared form expands over the ext's declared languages, mirroring how
   lowering expands it for the IR. *)
let instance_lang_names (body : Ast.ext_lib_body) (i : Ast.opaque_instance) :
    (string * string) list =
  match i.Ast.oi_names with
  | Ast.OnShared (name, _) ->
      List.map
        (fun (lp : Ast.lang_path) -> (lp.Ast.lp_lang, name))
        body.elib_langs
  | Ast.OnPerLang entries ->
      List.map
        (fun (e : Ast.opaque_name_entry) -> (e.one_lang, e.one_name))
        entries

(* The same instantiation (foreign name + argument, per language) declared
   twice across a module's "ext" library blocks: two tono types would both
   claim to be the same monomorphization, which the emitter cannot tell
   apart. *)
let check_instance_collisions (decls : Ast.decl list) : Diagnostic.t list =
  let seen = Hashtbl.create 8 in
  List.concat_map
    (fun (d : Ast.decl) ->
      match d.Ast.dkind with
      | Ast.DExtLib { body; _ } ->
          List.concat_map
            (fun (t : Ast.opaque_type) ->
              match t.Ast.opq_instance with
              | None -> []
              | Some i -> (
                  let arg = Printer.print_ty i.Ast.oi_arg in
                  let colliding =
                    List.filter_map
                      (fun (lang, foreign_name) ->
                        (* Keyed by the declaring "ext" too: the foreign name
                           is the library's own, not the user's, so two
                           different libraries that each export a "Source"
                           are not the same instantiation and must not
                           collide. *)
                        let key = (d.Ast.dname, lang, foreign_name, arg) in
                        if Hashtbl.mem seen key then Some (lang, foreign_name)
                        else (
                          Hashtbl.add seen key ();
                          None))
                      (instance_lang_names body i)
                  in
                  (* One diagnostic per instantiation, not one per language:
                     a shared-name duplicate would otherwise repeat the same
                     message once per declared language at the same span. *)
                  match colliding with
                  | [] -> []
                  | (lang, foreign_name) :: _ ->
                      [
                        err Error_codes.instance_duplicate i.Ast.oi_span
                          "the instantiation of '%s.%s' with '%s' is already \
                           declared elsewhere in this module for '%s'"
                          d.Ast.dname foreign_name arg lang;
                      ]))
            body.Ast.elib_types
      | _ -> [])
    decls

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
          let handles =
            List.map
              (fun (t : Ast.opaque_type) -> t.Ast.opq_name)
              body.Ast.elib_types
          in
          let free_diags =
            List.concat_map
              (check_extern ~tbl structs ~handles ~is_method:false)
              body.Ast.elib_externs
          in
          let method_diags =
            List.concat_map
              (fun (t : Ast.opaque_type) ->
                List.concat_map
                  (check_extern ~tbl structs ~handles ~is_method:true)
                  t.Ast.opq_methods)
              body.Ast.elib_types
          in
          let instance_diags =
            List.concat_map (check_instance_arg ~tbl) body.Ast.elib_types
          in
          let name_diags =
            List.concat_map (check_instance_names body) body.Ast.elib_types
          in
          free_diags @ method_diags @ instance_diags @ name_diags
      | _ -> [])
    decls
  @ check_foreign_name_collisions ~tbl decls
  @ check_instance_collisions decls
