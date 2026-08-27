(* Internal-consistency typecheck for the "ext" FFI library block, per op,
   per language binding: a call: arg naming an undeclared logical parameter
   (TC0070); a ctor literal projected into a declared foreign struct
   disagreeing in field name or type with the parameter it forwards
   (TC0071); a yields: position nothing consumes, neither a returns: ref nor
   the op's own return (TC0072); more than one "error"-typed yields:
   position (TC0073); a returns: with no yields: to project from (TC0074); a
   returns: that builds a type other than the op's own declared logical
   return (TC0075); a returns: field ref whose head is not a declared
   yields: name (TC0076); @errors naming a type that does not resolve
   (TC0077); a logical parameter this language's call: never consumes
   (TC0078); a language block that does not fit its struct (TC0092, TC0095,
   TC0097); @async naming a target without an asynchronous call (TC0093); a
   trait the boundary does not accept (TC0096); a bare name that is both a
   parameter and a class reference, a handle or a module struct (TC0098); a
   returns: building an opaque handle (TC0099).

   The cross-file closed accounting (decision K: TC0079-TC0081) lives in
   [Check_ext_lib_project], split out to keep this file under the line-count
   cap.

   Never verifies that a declared foreign spelling really exists in the
   target library: that is the target compiler's own job, out of scope here.
   The foreign-role wire/surface boundary lives in [Roles]/[Check_entries]
   instead, since it reuses their existing closed-boundary machinery. *)

let err code span fmt = Printf.ksprintf (Diagnostic.error ~code span) fmt

(* ── Parameter reference collection ────────────────────────────────────── *)

(* A call: arg or ctor field value that names a logical parameter, walked
   recursively through nested ctors and nested "ns.fn(...)" calls (their
   arguments can reference this op's own parameters too). *)
let rec collect_call_arg : Ast.call_arg -> string list = function
  | Ast.CaParam (n, _) | Ast.CaParamAs (n, _, _, _) -> [ n ]
  | Ast.CaRef _ | Ast.CaLit _ | Ast.CaForeign _ -> []
  | Ast.CaCtor c | Ast.CaCtorAs (c, _, _) ->
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

(* Every unknown-name diagnostic, walked the same way, anchored at the best
   span available (the arg's own span, or its ctor field's key span for a
   nested [Ast.trait_arg] which carries none of its own). A bare name may
   also be a class reference: an opaque handle of the block or one of the
   module's own structs ([classes], see [Roles.class_structs]); one that is
   both a parameter and a class reference is ambiguous (TC0098). *)
let rec unknown_param_call_arg ~(declared : string list)
    ~(handles : string list) ~(classes : string list) :
    Ast.call_arg -> Diagnostic.t list = function
  | Ast.CaParam (n, span) ->
      let is_param = List.mem n declared and is_handle = List.mem n handles in
      let is_class = List.mem n classes in
      if is_param && (is_handle || is_class) then
        [
          err Error_codes.extern_name_ambiguous span
            "'%s' is both a logical parameter of this op and %s; rename one so \
             the call names one thing"
            n
            (if is_handle then "an opaque handle of this ext block"
             else "a struct of this module");
        ]
      else if is_param || is_handle || is_class then []
      else
        [
          err Error_codes.extern_call_unknown_param span
            "'%s' is not a declared logical parameter of this op (nor an \
             opaque handle of this ext block, nor a struct of this module)"
            n;
        ]
  | Ast.CaParamAs (n, span, _, _) ->
      if List.mem n declared then []
      else
        [
          err Error_codes.extern_call_unknown_param span
            "'%s' is not a declared logical parameter of this op" n;
        ]
  | Ast.CaRef _ | Ast.CaLit _ | Ast.CaForeign _ -> []
  | Ast.CaCtor c | Ast.CaCtorAs (c, _, _) ->
      List.concat_map
        (fun (_, span, v) -> unknown_param_trait_arg declared span v)
        c.Ast.ctor_fields
  | Ast.CaCall nc ->
      List.concat_map
        (unknown_param_call_arg ~declared ~handles ~classes)
        nc.Ast.nc_args
  | Ast.CaList (items, _) ->
      List.concat_map (unknown_param_call_arg ~declared ~handles ~classes) items

and unknown_param_trait_arg (declared : string list) (span : Span.span) :
    Ast.trait_arg -> Diagnostic.t list = function
  | Ast.AName n ->
      if List.mem n declared then []
      else
        [
          err Error_codes.extern_call_unknown_param span
            "'%s' is not a declared logical parameter of this op" n;
        ]
  | Ast.AKv (_, v) -> unknown_param_trait_arg declared span v
  | Ast.AList xs -> List.concat_map (unknown_param_trait_arg declared span) xs
  | Ast.ACtor c ->
      List.concat_map
        (fun (_, fspan, v) -> unknown_param_trait_arg declared fspan v)
        c.Ast.ctor_fields
  | Ast.ACall ce ->
      List.concat_map
        (unknown_param_call_arg ~declared ~handles:[] ~classes:[])
        ce.Ast.ce_args
  | Ast.AString _ | Ast.AInt _ | Ast.AFloat _ | Ast.ARef _ -> []

(* ── One op's per-language rules ────────────────────────────────────────── *)

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
   e.g. [#(WithPrecision)(opts { retries: 3 })]). *)
let rec check_ctor_projection_arg
    (structs : (string, Ast.foreign_struct) Hashtbl.t)
    (params : Ast.extern_param list) : Ast.call_arg -> Diagnostic.t list =
  function
  | Ast.CaCtor c | Ast.CaCtorAs (c, _, _) ->
      check_ctor_projection structs params c
  | Ast.CaCall nc ->
      List.concat_map (check_ctor_projection_arg structs params) nc.Ast.nc_args
  | Ast.CaList (items, _) ->
      List.concat_map (check_ctor_projection_arg structs params) items
  | Ast.CaParam _ | Ast.CaParamAs _ | Ast.CaRef _ | Ast.CaLit _
  | Ast.CaForeign _ ->
      []

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

(* Every typed yields: position must be consumed: by a returns: ref reading
   it, or, when the binding projects nothing, by the op's own return (the
   position already is the declared logical type, so the list is the
   call's whole signature and the value needs no reader). The reserved
   "error" position is consumed by the boundary itself (it feeds the op's
   declared errors and the contract wrap), and a position under a foreign
   spelling is the value the target coerces into the declared return, so
   neither needs a reader either. *)
let check_yields_consumption (ed_return : Ast.ty) (b : Ast.extern_lang_body) :
    Diagnostic.t list =
  match b.Ast.elb_yields with
  | None -> []
  | Some ys ->
      let consumed = consumed_heads b.Ast.elb_returns in
      let is_the_return (t : Ast.ty) =
        Option.is_none b.Ast.elb_returns
        && String.equal (Printer.print_ty t) (Printer.print_ty ed_return)
      in
      List.concat_map
        (fun (y : Ast.yields_pos) ->
          match y.Ast.yp_ty with
          | Ast.YError _ | Ast.YForeign _ -> []
          | Ast.YType t ->
              if List.mem y.Ast.yp_name consumed || is_the_return t then []
              else
                [
                  err Error_codes.extern_yields_position_dead y.Ast.yp_name_span
                    "yields position '%s' is never consumed: no 'returns:' \
                     reads it and it is not the op's own return '%s'"
                    y.Ast.yp_name
                    (Printer.print_ty ed_return);
                ])
        ys

(* A returns: cannot build an opaque handle: a handle has no fields to
   project into, it is what the call itself returns. The binding declares
   the call's positions with yields: alone. *)
let check_returns_not_handle ~(handles : string list) (b : Ast.extern_lang_body)
    : Diagnostic.t list =
  match b.Ast.elb_returns with
  | Some rl when List.mem (Printer.print_ty rl.Ast.rl_type) handles ->
      [
        err Error_codes.extern_returns_handle rl.Ast.rl_span
          "'returns:' builds '%s', an opaque handle: a handle is what the call \
           returns, never a projection; declare the call's positions with \
           'yields:' alone"
          (Printer.print_ty rl.Ast.rl_type);
      ]
  | _ -> []

(* At most one yields: position may be the reserved "error" type. *)
let check_single_error_position (ys : Ast.yields_pos list) : Diagnostic.t list =
  let errs =
    List.filter
      (fun (y : Ast.yields_pos) ->
        match y.Ast.yp_ty with Ast.YError _ -> true | _ -> false)
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

(* returns: builds the op's own declared logical type, never another one:
   "Tipo { ... }" means the same thing here as everywhere else in tono. *)
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
            "'returns:' builds '%s', but the op's declared logical return is \
             '%s'"
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

(* The arguments a language block's call: line passes: the call's own and
   the chained method's, which the rules over arguments treat alike. *)
let call_line_args (b : Ast.extern_lang_body) : Ast.call_arg list =
  b.Ast.elb_call_args
  @ match b.Ast.elb_call_chain with None -> [] | Some nc -> nc.Ast.nc_args

(* Every logical parameter must be consumed by this language's own call:,
   recursively through any ctor projection or nested call. *)
let check_param_consumption (params : Ast.extern_param list)
    (b : Ast.extern_lang_body) : Diagnostic.t list =
  let referenced = List.concat_map collect_call_arg (call_line_args b) in
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

(* ── An op's traits ─────────────────────────────────────────────────────── *)

(* @async lists targets where the foreign call is asynchronous: each must
   have the concept and a module path declared by this ext. @errors lists
   declared error shapes. Anything else is not a trait of the boundary. *)
let check_extern_traits ~(tbl : Symtab.t) ~(langs : string list)
    (e : Ast.extern_decl) : Diagnostic.t list =
  List.concat_map
    (fun (t : Ast.trait) ->
      match t.Ast.tname with
      | "async" ->
          (match t.Ast.targs with
            | [] ->
                [
                  err Error_codes.extern_async_target_invalid t.Ast.tspan
                    "@async on an ext op lists the targets where the foreign \
                     call is asynchronous (e.g. @async(rust)); absence means \
                     synchronous";
                ]
            | _ -> [])
          @ List.concat_map
              (function
                | Ast.AName lang ->
                    if not (List.mem lang Ext_lib_vocab.async_targets) then
                      [
                        err Error_codes.extern_async_target_invalid t.Ast.tspan
                          "'%s' has no asynchronous call to declare; @async \
                           names one of %s"
                          lang
                          (Ext_lib_vocab.quoted Ext_lib_vocab.async_targets);
                      ]
                    else if not (List.mem lang langs) then
                      [
                        err Error_codes.extern_async_target_invalid t.Ast.tspan
                          "the ext declares no '%s' module path for @async to \
                           apply to"
                          lang;
                      ]
                    else []
                | _ ->
                    [
                      err Error_codes.extern_async_target_invalid t.Ast.tspan
                        "@async expects target names";
                    ])
              t.Ast.targs
      | "errors" ->
          List.concat_map
            (function
              | Ast.AName n when Option.is_none (Symtab.find n tbl) ->
                  [
                    err Error_codes.extern_error_unknown t.Ast.tspan
                      "unknown error type '%s'; declare it before listing it \
                       in @errors"
                      n;
                  ]
              | Ast.AName _ -> []
              | _ ->
                  [
                    err Error_codes.extern_error_unknown t.Ast.tspan
                      "@errors expects type names";
                  ])
            t.Ast.targs
      | "doc" -> []
      | other ->
          [
            err Error_codes.extern_trait_invalid t.Ast.tspan
              "@%s is not a trait of an ext op; only %s apply here" other
              (Ext_lib_vocab.quoted Ext_lib_vocab.op_traits);
          ])
    e.Ast.ed_traits

let check_extern ~(tbl : Symtab.t) ~(langs : string list)
    (structs : (string, Ast.foreign_struct) Hashtbl.t) ~(handles : string list)
    ~(classes : string list) (e : Ast.extern_decl) : Diagnostic.t list =
  let declared =
    List.map (fun (p : Ast.extern_param) -> p.Ast.ep_name) e.Ast.ed_params
  in
  check_extern_traits ~tbl ~langs e
  @ List.concat_map
      (fun (b : Ast.extern_lang_body) ->
        List.concat_map
          (unknown_param_call_arg ~declared ~handles ~classes)
          (call_line_args b)
        @ List.concat_map
            (fun a -> check_ctor_projection_arg structs e.Ast.ed_params a)
            (call_line_args b)
        @ check_yields_consumption e.Ast.ed_return b
        @ (match b.Ast.elb_yields with
          | Some ys -> check_single_error_position ys
          | None -> [])
        @ check_returns_requires_yields b
        @ check_returns_type e.Ast.ed_return b
        @ check_returns_not_handle ~handles b
        @ check_returns_refs b
        @ check_param_consumption e.Ast.ed_params b)
      e.Ast.ed_langs

(* ── Language blocks on structs ─────────────────────────────────────────── *)

(* The blocks of one struct: each language at most once, each one the ext
   declares a module path for (or, at top level, a target at all), and
   each keyed entry a field of the struct; a handle carries no keyed entry. *)
let check_lang_blocks ~(allowed : string list) ~(allowed_what : string)
    ~(fields : string list) ~(is_handle : bool) (blocks : Ast.lang_block list) :
    Diagnostic.t list =
  let seen = Hashtbl.create 4 in
  List.concat_map
    (fun (b : Ast.lang_block) ->
      let dup =
        if Hashtbl.mem seen b.Ast.lb_lang then
          [
            err Error_codes.lang_block_mismatch b.Ast.lb_lang_span
              "this struct already has a '%s' block" b.Ast.lb_lang;
          ]
        else (
          Hashtbl.add seen b.Ast.lb_lang ();
          [])
      in
      let known =
        if List.mem b.Ast.lb_lang allowed then []
        else
          [
            err Error_codes.lang_block_mismatch b.Ast.lb_lang_span
              "%s '%s'; a language block names one of %s" allowed_what
              b.Ast.lb_lang
              (Ext_lib_vocab.quoted allowed);
          ]
      in
      let keyed =
        List.concat_map
          (fun (n, span, _, _) ->
            if is_handle then
              [
                err Error_codes.lang_block_field_unknown span
                  "an opaque handle has no fields; its '%s' block is only the \
                   storage type"
                  b.Ast.lb_lang;
              ]
            else if List.mem n fields then []
            else
              [
                err Error_codes.lang_block_field_unknown span
                  "'%s' is not a field of this struct" n;
              ])
          b.Ast.lb_fields
      in
      dup @ known @ keyed)
    blocks

(* A top-level struct's language blocks belong to an error struct only:
   there they say how the target recognizes the foreign error. *)
let is_error_struct (d : Ast.decl) : bool =
  List.exists
    (fun (t : Ast.trait) ->
      match t.Ast.tname with "status" | "errorCode" -> true | _ -> false)
    d.Ast.dtraits

let check_struct_lang_blocks (decls : Ast.decl list) : Diagnostic.t list =
  List.concat_map
    (fun (d : Ast.decl) ->
      match d.Ast.dkind with
      | Ast.DStruct { slangs = []; _ } -> []
      | Ast.DStruct { members; slangs; _ } ->
          let misplaced =
            if is_error_struct d then []
            else
              List.map
                (fun (b : Ast.lang_block) ->
                  err Error_codes.struct_lang_block_misplaced b.Ast.lb_span
                    "a language block on '%s' has nothing to bind: only an \
                     error struct (@status or @errorCode) declares how a \
                     target recognizes it"
                    d.Ast.dname)
                slangs
          in
          misplaced
          @ check_lang_blocks ~allowed:Ext_lib_vocab.targets
              ~allowed_what:"no target is named"
              ~fields:(List.map (fun (m : Ast.member) -> m.Ast.mname) members)
              ~is_handle:false slangs
      | _ -> [])
    decls

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

(* ── Per-module entry point ────────────────────────────────────────────── *)

let check_decls ~(tbl : Symtab.t) (decls : Ast.decl list) : Diagnostic.t list =
  let classes = Roles.class_structs (Roles.classify decls) decls in
  List.concat_map
    (fun (d : Ast.decl) ->
      match d.Ast.dkind with
      | Ast.DExtLib { body; _ } ->
          let langs =
            List.map
              (fun (lp : Ast.lang_path) -> lp.Ast.lp_lang)
              body.elib_langs
          in
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
          (* Whether the ext declares a module path for the block's
             language is a cross-file question (decision K), answered in
             [Check_ext_lib_project]; here only the target set is closed. *)
          let blocks_of ~fields ~is_handle blocks =
            check_lang_blocks ~allowed:Ext_lib_vocab.targets
              ~allowed_what:"no target is named" ~fields ~is_handle blocks
          in
          List.concat_map
            (check_extern ~tbl ~langs structs ~handles ~classes)
            body.Ast.elib_externs
          @ List.concat_map
              (fun (t : Ast.opaque_type) ->
                blocks_of ~fields:[] ~is_handle:true t.Ast.opq_langs
                @ List.concat_map
                    (check_extern ~tbl ~langs structs ~handles ~classes)
                    t.Ast.opq_methods)
              body.Ast.elib_types
          @ List.concat_map
              (fun (s : Ast.foreign_struct) ->
                blocks_of
                  ~fields:
                    (List.map
                       (fun (f : Ast.foreign_field) -> f.Ast.ff_name)
                       s.Ast.fs_fields)
                  ~is_handle:false s.Ast.fs_langs)
              body.Ast.elib_structs
      | _ -> [])
    decls
  @ check_foreign_name_collisions ~tbl decls
  @ check_struct_lang_blocks decls
