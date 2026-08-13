(* Entry-model validation. Roles emerge from struct content (Roles.classify)
   and their boundaries are closed: an entry/config never crosses the wire
   (TC0034) and a wire position never carries construction metadata (TC0035).
   Field values come only from declared sources; a source that can never fire is
   dead (TC0036). Resolution is lazy: a consumed field whose chain reaches a
   field with no declared source errors at the point of consumption, naming the
   chain (TC0037); references must resolve (TC0038) and the resolution graph
   must be a DAG (TC0039). Selection is a literal table: malformed matches are
   TC0040 and non-exhaustive ones TC0041. Composition happens only at the
   composition point (TC0042). Protocol traits obey their positions: an entry
   @http op names its endpoint (TC0043) and @header/@timeout/@retry require
   @http, refs require an entry (TC0044). Transform catalogs are closed
   (TC0045), and entry/config shapes take no generics or nullable fields
   (TC0046). *)

let err code span fmt = Printf.ksprintf (Diagnostic.error ~code span) fmt

let find_trait name (traits : Ast.trait list) : Ast.trait option =
  List.find_opt (fun (t : Ast.trait) -> String.equal t.tname name) traits

let traits_named name (traits : Ast.trait list) : Ast.trait list =
  List.filter (fun (t : Ast.trait) -> String.equal t.tname name) traits

(* The closed @str::* catalog, shared vocabulary with the casing engine. *)
let str_transforms =
  [ "trim"; "upper_snake"; "snake"; "kebab"; "pascal"; "lower"; "upper" ]

let is_nullable : Ast.ty -> bool = function
  | Ast.TNullable _ -> true
  | _ -> false

(* The field scope, reference collection, and resolution graph live in
   [Entry_scope]; match validation lives in [Check_entry_match]. *)
let base_ty = Entry_scope.base_ty
let source_traits = Entry_scope.source_traits
let has_own_source = Entry_scope.has_own_source
let struct_members = Entry_scope.struct_members
let role_of_name = Entry_scope.role_of_name
let resolve_path = Entry_scope.resolve_path
let path_str = Entry_scope.path_str
let scalar_of_ty = Entry_scope.scalar_of_ty
let member_trait_refs = Entry_scope.member_trait_refs
let op_refs = Entry_scope.op_refs
let dep_heads = Entry_scope.dep_heads
let check_cycles = Entry_scope.check_cycles
let broken = Entry_scope.broken
let chain_error = Entry_scope.chain_error
let check_match = Check_entry_match.check_match

(* ── Field-local rules ─────────────────────────────────────────────────── *)

let check_transforms (m : Ast.member) : Diagnostic.t list =
  List.filter_map
    (fun (tr : Ast.trait) ->
      match String.index_opt tr.Ast.tname ':' with
      | None -> None
      | Some _ -> (
          match Lower.transform_of tr.tname with
          | Some t when List.mem t str_transforms -> None
          | Some t ->
              Some
                (err Error_codes.transform_unknown tr.tspan
                   "unknown @str transform '%s'; the catalog is: %s" t
                   (String.concat ", " str_transforms))
          | None ->
              Some
                (err Error_codes.transform_unknown tr.tspan
                   "unknown transform catalog in '@%s'; only '@str::*' exists"
                   tr.tname)))
    m.Ast.mtraits

(* Source-combination rules: @arg excludes everything else; a match or a
   @format is itself the way the field gets its value, so combining either
   with sources (or each other) leaves dead sources. *)
let check_source_combinations ~in_config (m : Ast.member) : Diagnostic.t list =
  let sources = source_traits m in
  let has name = Option.is_some (find_trait name m.mtraits) in
  let dead span what = err Error_codes.source_dead span "%s" what in
  let arg_diags =
    match find_trait "arg" m.mtraits with
    | Some tr
      when List.length sources > 1 || Option.is_some m.mvalue || has "format" ->
        [
          dead tr.tspan
            "@arg is explicit and exclusive: the other sources (or match / \
             @format) on this field can never fire";
        ]
    | _ -> []
  in
  let match_diags =
    match m.mvalue with
    | Some (Ast.MMatch fm)
      when (not (has "arg")) && (sources <> [] || has "format") ->
        [
          dead fm.match_span
            "a match is the field's only value; declare sources on the fields \
             its arms reference instead";
        ]
    | Some (Ast.MCall ce)
      when (not (has "arg"))
           (* @with is not a competing source here: paired with a call value
              it is the RFC-0023 injectable-handle idiom (decision G) --
              the caller may supply the value, construction is the fallback
              -- not a source that would leave the call's own value dead. *)
           && (List.exists
                 (fun (t : Ast.trait) -> not (String.equal t.Ast.tname "with"))
                 sources
              || has "format") ->
        [
          dead ce.ce_span
            "an extern call is the field's only value; declare sources on \
             other fields instead";
        ]
    | _ -> []
  in
  let format_diags =
    match find_trait "format" m.mtraits with
    | Some tr when (not (has "arg")) && Option.is_none m.mvalue && sources <> []
      ->
        [
          dead tr.tspan
            "@format derives the field's value; the sources on this field can \
             never fire";
        ]
    | _ -> []
  in
  let config_diags =
    if in_config then
      List.filter_map
        (fun (tr : Ast.trait) ->
          if List.mem tr.Ast.tname [ "arg"; "with" ] then
            Some
              (err Error_codes.source_position_invalid tr.tspan
                 "@%s lives on entry fields only; inside a config the sources \
                  are @env and @default"
                 tr.tname)
          else None)
        m.Ast.mtraits
    else []
  in
  arg_diags @ match_diags @ format_diags @ config_diags

(* A "{...}" inside an @env name would read as literal characters of the
   variable name and only surprise at runtime; deriving the name is @format's
   job. *)
let check_env_names (m : Ast.member) : Diagnostic.t list =
  List.filter_map
    (fun (tr : Ast.trait) ->
      match (tr.Ast.tname, tr.targs) with
      | "env", [ Ast.AString s ] when String.contains s '{' ->
          Some
            (err Error_codes.source_position_invalid tr.tspan
               "@env takes a literal variable name; derive a dynamic name with \
                @format on a named field and reference it with @env(.field)")
      | _ -> None)
    m.Ast.mtraits

(* @env(.field) names the environment variable through the field's value, so
   the referenced field must be a string. *)
let check_env_ref_types ctx (fields : Ast.member list) (m : Ast.member) :
    Diagnostic.t list =
  List.filter_map
    (fun (tr : Ast.trait) ->
      match (tr.Ast.tname, tr.targs) with
      | "env", [ Ast.ARef r ] -> (
          match resolve_path ctx fields r.segs with
          | Some f when scalar_of_ty ctx f.mtype <> Entry_scope.SString ->
              Some
                (err Error_codes.source_position_invalid tr.tspan
                   "@env(%s) must reference a string field: the value names \
                    the environment variable"
                   (path_str r.segs))
          | _ -> None (* unknown refs are reported by the reachability pass *))
      | _ -> None)
    m.Ast.mtraits

let check_nullable_field (m : Ast.member) : Diagnostic.t list =
  if is_nullable m.mtype then
    [
      err Error_codes.entry_shape_invalid m.mname_span
        "field '%s' cannot be nullable here: presence of an entry/config field \
         is governed by its sources (@with for optional, @default for a \
         fallback)"
        m.mname;
    ]
  else []

(* ── Composition (@bind) ───────────────────────────────────────────────── *)

let check_binds ctx (fields : Ast.member list) (m : Ast.member) :
    Diagnostic.t list =
  let binds = traits_named "bind" m.mtraits in
  if binds = [] then []
  else
    match base_ty m.mtype with
    | Ast.TName (n, [], _) when role_of_name ctx n = Roles.Config ->
        let config_fields = Option.value ~default:[] (struct_members ctx n) in
        List.concat_map
          (fun (tr : Ast.trait) ->
            match tr.Ast.targs with
            | [ Ast.AName target; Ast.ARef r ] -> (
                let target_field =
                  List.find_opt
                    (fun (cm : Ast.member) -> String.equal cm.mname target)
                    config_fields
                in
                let source_field = resolve_path ctx fields r.segs in
                (match target_field with
                  | Some _ -> []
                  | None ->
                      [
                        err Error_codes.bind_invalid tr.tspan
                          "'%s' is not a field of the composed config '%s'"
                          target n;
                      ])
                @ (match source_field with
                  | Some _ -> []
                  | None ->
                      [
                        err Error_codes.field_ref_unknown tr.tspan
                          "unknown field '%s' as a @bind source"
                          (path_str r.segs);
                      ])
                @
                (* Binds are typed: the bound value replaces the config
                   field's own resolution, so the types must agree. The
                   printed spelling is the structural comparison. *)
                match (target_field, source_field) with
                | Some tf, Some sf
                  when not
                         (String.equal
                            (Printer.print_ty (base_ty tf.mtype))
                            (Printer.print_ty (base_ty sf.mtype))) ->
                    [
                      err Error_codes.bind_invalid tr.tspan
                        "@bind source '%s' has type %s but config field '%s' \
                         is %s"
                        (path_str r.segs)
                        (Printer.print_ty (base_ty sf.mtype))
                        target
                        (Printer.print_ty (base_ty tf.mtype));
                    ]
                | _ -> [])
            | _ ->
                (* Malformed argument shapes are diagnosed during lowering. *)
                [])
          binds
    | _ ->
        List.map
          (fun (tr : Ast.trait) ->
            err Error_codes.bind_invalid tr.Ast.tspan
              "@bind only lives at a composition point: an entry field whose \
               type is a config")
          binds

(* ── Operations: the position rules live in [Check_entry_ops] ──────────── *)

let check_loose_op = Check_entry_ops.check_loose_op
let check_entry_op = Check_entry_ops.check_entry_op

(* ── Wire boundary ─────────────────────────────────────────────────────── *)

(* Why a name classifies as a config when it declares no sources of its own:
   the entry composition that flipped it. Boundary diagnostics cite this so
   the error points at the cause, not only the wire use. *)
let config_origin ctx (name : string) : string =
  let flipped_by =
    match Entry_scope.decl_by_name ctx name with
    | Some { Ast.dkind = Ast.DStruct { members; _ }; _ }
      when not (List.exists Roles.member_has_source members) ->
        List.find_map
          (fun (d : Ast.decl) ->
            match d.Ast.dkind with
            | Ast.DStruct { ops = _ :: _; members = emembers; _ } ->
                List.find_map
                  (fun (m : Ast.member) ->
                    match base_ty m.mtype with
                    | Ast.TName (n, [], _)
                      when String.equal n name && Roles.member_has_bind m ->
                        Some (d.Ast.dname, m.mname)
                    | _ -> None)
                  emembers
            | _ -> None)
          ctx.Entry_scope.decls
    | _ -> None
  in
  match flipped_by with
  | Some (entry, field) ->
      Printf.sprintf
        " (a config because entry '%s' composes it via @bind on field '%s')"
        entry field
  | None -> ""

(* Any type reference to an entry/config outside its closed boundary: op
   inputs and outputs, declared errors, wire struct members, union payloads,
   and anything nested in lists, maps, or generic arguments. The only legal
   appearance (a bare config as a direct entry field, the composition point)
   is carved out by the caller before recursing here. *)
let rec boundary_ty ctx (t : Ast.ty) : Diagnostic.t list =
  match t with
  | Ast.TName (n, args, span) ->
      (match role_of_name ctx n with
        | Roles.Entry ->
            [
              err Error_codes.entry_wire_boundary span
                "'%s' is an entry and cannot be referenced as a type" n;
            ]
        | Roles.Config ->
            [
              err Error_codes.entry_wire_boundary span
                "'%s' is a config and can only be composed directly by an \
                 entry field%s"
                n (config_origin ctx n);
            ]
        | Roles.Wire -> []
        | Roles.Foreign ->
            [
              err Error_codes.entry_wire_boundary span
                "'%s' is a foreign form declared in an 'ext' library block and \
                 can never be wire, an op input/output, or public surface"
                n;
            ])
      @ List.concat_map (boundary_ty ctx) args
  | Ast.TQName (qualifier, n, args, span) ->
      (if role_of_name ctx (qualifier ^ "." ^ n) = Roles.Foreign then
         [
           err Error_codes.entry_wire_boundary span
             "'%s.%s' is a foreign handle declared in an 'ext' library block \
              and can only be composed directly by an entry field"
             qualifier n;
         ]
       else [])
      @ List.concat_map (boundary_ty ctx) args
  | Ast.TList (t, _) | Ast.TNullable (t, _) -> boundary_ty ctx t
  | Ast.TMap (k, v, _) -> boundary_ty ctx k @ boundary_ty ctx v
  | Ast.TPrim _ | Ast.TError _ -> []

let check_op_boundary ctx (op : Ast.decl) : Diagnostic.t list =
  match op.dkind with
  | Ast.DOp { pname = _; input; output } ->
      let opt = function Some t -> boundary_ty ctx t | None -> [] in
      let error_diags =
        List.concat_map
          (fun (tr : Ast.trait) ->
            if String.equal tr.Ast.tname "errors" then
              List.filter_map
                (function
                  | Ast.AName n
                    when role_of_name ctx n = Roles.Entry
                         || role_of_name ctx n = Roles.Config ->
                      Some
                        (err Error_codes.entry_wire_boundary tr.tspan
                           "'%s' is not a wire error type: an entry/config \
                            never crosses the wire"
                           n)
                  | _ -> None)
                tr.targs
            else [])
          op.dtraits
      in
      opt input @ opt output @ error_diags
  | _ -> []

(* Construction metadata in positions that can never carry it: union variants
   and enum cases. *)
let check_non_struct_sources (d : Ast.decl) : Diagnostic.t list =
  let flag (traits : Ast.trait list) =
    List.filter_map
      (fun (tr : Ast.trait) ->
        if Roles.source_marker tr.Ast.tname then
          Some
            (err Error_codes.source_position_invalid tr.tspan
               "@%s is a construction source and only lives on entry/config \
                fields"
               tr.tname)
        else if String.equal tr.Ast.tname "bind" then
          Some
            (err Error_codes.bind_invalid tr.tspan
               "@bind only lives at a composition point: an entry field whose \
                type is a config")
        else None)
      traits
  in
  match d.dkind with
  | Ast.DUnion { variants; _ } ->
      List.concat_map (fun (v : Ast.union_variant) -> flag v.vtraits) variants
  | Ast.DEnum { cases } ->
      List.concat_map (fun (c : Ast.enum_case) -> flag c.ctraits) cases
  | _ -> []

(* ── Entries and configs ───────────────────────────────────────────────── *)

let check_generics (d : Ast.decl) params what : Diagnostic.t list =
  if params = [] then []
  else
    [
      err Error_codes.entry_shape_invalid d.dname_span
        "%s '%s' cannot be generic: the construction surface is concrete" what
        d.dname;
    ]

(* Member types never reference an entry, and reference a config only as the
   bare type of a direct entry field (the composition point); a config nested
   in a list, map, or generic argument is closed like any wire position. A
   foreign opaque handle gets the same one carve-out: a bare qualified field
   (bus: companybus.publisher) is the injectable-handle composition point the
   RFC describes, but nested/wire uses stay closed like any config. *)
let check_member_boundary ctx ~(container : Roles.role) (m : Ast.member) :
    Diagnostic.t list =
  match container with
  | Roles.Entry -> (
      match base_ty m.mtype with
      | Ast.TName (n, [], _) when role_of_name ctx n = Roles.Config -> []
      | Ast.TQName (q, n, [], _)
        when role_of_name ctx (q ^ "." ^ n) = Roles.Foreign ->
          []
      | t -> boundary_ty ctx t)
  | Roles.Config | Roles.Wire | Roles.Foreign -> boundary_ty ctx m.mtype

(* Construction metadata on a wire member would be dropped in silence: the IR
   loses a match, and @format/@str::* would ride the bag with no consumer,
   while the source reads as if they fire. *)
let check_wire_member_metadata (m : Ast.member) : Diagnostic.t list =
  (match m.mvalue with
    | Some (Ast.MMatch fm) ->
        [
          err Error_codes.source_position_invalid fm.match_span
            "a match selection only lives on entry/config fields";
        ]
    | Some (Ast.MCall ce) ->
        [
          err Error_codes.source_position_invalid ce.ce_span
            "an extern call only lives on entry/config fields";
        ]
    | None -> [])
  @ List.filter_map
      (fun (tr : Ast.trait) ->
        if String.equal tr.Ast.tname "format" then
          Some
            (err Error_codes.source_position_invalid tr.tspan
               "@format only lives on entry/config fields")
        else if Option.is_some (String.index_opt tr.Ast.tname ':') then
          Some
            (err Error_codes.source_position_invalid tr.tspan
               "transform catalogs (@str::*) only live on entry/config fields")
        else if String.equal tr.Ast.tname "bind" then
          Some
            (err Error_codes.bind_invalid tr.tspan
               "@bind only lives at a composition point: an entry field whose \
                type is a config")
        else None)
      m.Ast.mtraits

let check_entry ctx (d : Ast.decl) params (members : Ast.member list)
    (ops : Ast.decl list) : Diagnostic.t list =
  let field_diags =
    List.concat_map
      (fun (m : Ast.member) ->
        let unresolved_ref_diags =
          List.filter_map
            (fun (segs, span) ->
              if Option.is_some (resolve_path ctx members segs) then None
              else
                Some
                  (err Error_codes.field_ref_unknown span "unknown field '%s'"
                     (path_str segs)))
            (member_trait_refs m)
        in
        check_source_combinations ~in_config:false m
        @ check_env_names m
        @ check_env_ref_types ctx members m
        @ check_transforms m @ check_nullable_field m
        @ check_binds ctx members m
        @ check_member_boundary ctx ~container:Roles.Entry m
        @ unresolved_ref_diags
        @
        match m.mvalue with
        | Some (Ast.MMatch fm) -> check_match ctx members m fm
        (* An extern call's projection is checked against its ext block's
           declared signature; that resolution is deferred (out of scope). *)
        | Some (Ast.MCall _) | None -> [])
      members
  in
  let cycle_diags = check_cycles ctx members in
  (* Nested ops share the entry's id scope (module#entry.op): two ops with one
     name would lower to colliding shape ids. *)
  let dup_op_diags =
    let rec go seen = function
      | [] -> []
      | (op : Ast.decl) :: rest ->
          (if List.mem op.dname seen then
             [
               err Error_codes.duplicate_shape op.dname_span
                 "operation '%s' is declared twice in entry '%s'" op.dname
                 d.dname;
             ]
           else [])
          @ go (op.dname :: seen) rest
    in
    go [] ops
  in
  let op_diags = List.concat_map (check_entry_op ctx members) ops in
  (* Lazy resolution: a consumed chain that bottoms out on a sourceless field
     errors at the point of consumption, naming the chain. Sourceless fields
     nothing consumes error on the field itself. *)
  let broken_of = broken ctx members in
  let consumptions =
    List.concat_map op_refs ops
    @ List.concat_map
        (fun (m : Ast.member) ->
          List.filter_map
            (fun (tr : Ast.trait) ->
              match (tr.Ast.tname, tr.targs) with
              | "bind", [ Ast.AName _; Ast.ARef r ] ->
                  Some (r.Ast.segs, tr.tspan)
              | _ -> None)
            m.mtraits)
        members
  in
  let lazy_diags =
    (* One op can consume the same broken field through several traits; the
       identical chain reports once. *)
    List.sort_uniq compare
      (List.filter_map
         (fun ((segs : string list), span) ->
           match segs with
           | head :: _ when Option.is_some (resolve_path ctx members segs) ->
               Option.map (chain_error span segs) (broken_of head)
           | _ -> None)
         consumptions)
  in
  (* Fields transitively consumed by the ops/binds above are covered by the
     chain errors; the rest still must declare a source. *)
  let consumed =
    let rec close acc = function
      | [] -> acc
      | name :: rest when List.mem name acc -> close acc rest
      | name :: rest -> (
          match
            List.find_opt
              (fun (m : Ast.member) -> String.equal m.mname name)
              members
          with
          | None -> close acc rest
          | Some m -> close (name :: acc) (dep_heads ctx members m @ rest))
    in
    close []
      (List.filter_map
         (fun ((segs : string list), _) ->
           match segs with head :: _ -> Some head | [] -> None)
         consumptions)
  in
  let sourceless_diags =
    List.filter_map
      (fun (m : Ast.member) ->
        if has_own_source ctx m || List.mem m.mname consumed then None
        else
          Some
            (err Error_codes.field_unresolvable m.mname_span
               "field '%s' declares no value source (@arg, @with, @env, \
                @default, a match, or @format)"
               m.mname))
      members
  in
  check_generics d params "entry"
  @ field_diags @ cycle_diags @ dup_op_diags @ op_diags @ lazy_diags
  @ sourceless_diags

(* Config fields a composing entry binds; a sourceless bound field is fed at
   the composition point, so it is not an error. *)
let bound_config_fields ctx (config_name : string) : string list =
  List.concat_map
    (fun (d : Ast.decl) ->
      match d.Ast.dkind with
      | Ast.DStruct { ops = _ :: _; members; _ } ->
          List.concat_map
            (fun (m : Ast.member) ->
              match base_ty m.mtype with
              | Ast.TName (n, [], _) when String.equal n config_name ->
                  List.filter_map
                    (fun (tr : Ast.trait) ->
                      match (tr.Ast.tname, tr.targs) with
                      | "bind", [ Ast.AName target; Ast.ARef _ ] -> Some target
                      | _ -> None)
                    m.mtraits
              | _ -> [])
            members
      | _ -> [])
    ctx.Entry_scope.decls

let check_config ctx (d : Ast.decl) params (members : Ast.member list) :
    Diagnostic.t list =
  let bound = bound_config_fields ctx d.dname in
  let field_diags =
    List.concat_map
      (fun (m : Ast.member) ->
        let unresolved_ref_diags =
          List.filter_map
            (fun (segs, span) ->
              if Option.is_some (resolve_path ctx members segs) then None
              else
                Some
                  (err Error_codes.field_ref_unknown span "unknown field '%s'"
                     (path_str segs)))
            (member_trait_refs m)
        in
        let sourceless =
          if has_own_source ctx m || List.mem m.mname bound then []
          else
            [
              err Error_codes.field_unresolvable m.mname_span
                "config field '%s' declares no value source (@env/@default) \
                 and no entry binds it"
                m.mname;
            ]
        in
        check_source_combinations ~in_config:true m
        @ check_env_names m
        @ check_env_ref_types ctx members m
        @ check_transforms m @ check_nullable_field m
        @ check_member_boundary ctx ~container:Roles.Config m
        @ (traits_named "bind" m.mtraits
          |> List.map (fun (tr : Ast.trait) ->
              err Error_codes.bind_invalid tr.Ast.tspan
                "@bind only lives at a composition point: an entry field whose \
                 type is a config"))
        @ unresolved_ref_diags @ sourceless
        @
        match m.mvalue with
        | Some (Ast.MMatch fm) -> check_match ctx members m fm
        | Some (Ast.MCall _) | None -> [])
      members
  in
  check_generics d params "config" @ check_cycles ctx members @ field_diags

(* ── Module pass ───────────────────────────────────────────────────────── *)

let check_decls (decls : Ast.decl list) : Diagnostic.t list =
  let ctx = { Entry_scope.decls; roles = Roles.classify decls } in
  List.concat_map
    (fun (d : Ast.decl) ->
      match d.dkind with
      | Ast.DStruct { params; members; ops } -> (
          match Roles.role_of ctx.roles d.dname with
          | Roles.Entry ->
              check_entry ctx d params members ops
              @ List.concat_map (check_op_boundary ctx) ops
          | Roles.Config -> check_config ctx d params members
          | Roles.Wire ->
              List.concat_map
                (fun m ->
                  check_member_boundary ctx ~container:Roles.Wire m
                  @ check_wire_member_metadata m)
                members
          (* A DStruct's own name is never classified Foreign: only a
             foreign_struct/opaque_type declared inside an "ext" block is,
             and those are never DStruct decls. Kept for exhaustiveness. *)
          | Roles.Foreign -> [])
      | Ast.DUnion { variants; _ } ->
          check_non_struct_sources d
          @ List.concat_map
              (fun (v : Ast.union_variant) ->
                match v.vpayload with Some t -> boundary_ty ctx t | None -> [])
              variants
      | Ast.DEnum _ -> check_non_struct_sources d
      | Ast.DOp _ -> check_op_boundary ctx d @ check_loose_op ctx d
      | Ast.DExt _ | Ast.DExtLib _ | Ast.DTest _ -> [])
    decls
