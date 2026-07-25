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

let kv_arg key (args : Ast.trait_arg list) : Ast.trait_arg option =
  List.find_map
    (function Ast.AKv (k, v) when String.equal k key -> Some v | _ -> None)
    args

let source_names = [ "arg"; "with"; "env"; "default" ]

let source_traits (m : Ast.member) : Ast.trait list =
  List.filter
    (fun (t : Ast.trait) -> List.mem t.Ast.tname source_names)
    m.Ast.mtraits

(* The closed @str::* catalog, shared vocabulary with the casing engine. *)
let str_transforms =
  [ "trim"; "upper_snake"; "snake"; "kebab"; "pascal"; "lower"; "upper" ]

let is_nullable : Ast.ty -> bool = function
  | Ast.TNullable _ -> true
  | _ -> false

let base_ty : Ast.ty -> Ast.ty = function Ast.TNullable (t, _) -> t | t -> t

(* ── Context ───────────────────────────────────────────────────────────── *)

type ctx = { decls : Ast.decl list; roles : (string, Roles.role) Hashtbl.t }

let decl_by_name ctx name =
  List.find_opt (fun (d : Ast.decl) -> String.equal d.Ast.dname name) ctx.decls

let struct_members ctx name : Ast.member list option =
  match decl_by_name ctx name with
  | Some { dkind = Ast.DStruct { members; _ }; _ } -> Some members
  | _ -> None

let role_of_name ctx name : Roles.role = Roles.role_of ctx.roles name

(* Resolve a reference path against [fields]: the head names a field, further
   segments descend into struct-typed fields (a structured source or a composed
   config). Returns the terminal member when every segment resolves. *)
let rec resolve_path ctx (fields : Ast.member list) (segs : string list) :
    Ast.member option =
  match segs with
  | [] -> None
  | s :: rest -> (
      match
        List.find_opt (fun (m : Ast.member) -> String.equal m.mname s) fields
      with
      | None -> None
      | Some m -> (
          if rest = [] then Some m
          else
            match base_ty m.mtype with
            | Ast.TName (n, [], _) -> (
                match struct_members ctx n with
                | Some ms -> resolve_path ctx ms rest
                | None -> None)
            | _ -> None))

let path_str segs = "." ^ String.concat "." segs

(* ── Scalar classification (match subjects and patterns) ───────────────── *)

type scalar = SBool | SString | SInt | SEnum of string list | SOther

let int_prims = [ "i8"; "i16"; "i32"; "i64"; "u8"; "u16"; "u32"; "u64" ]

let scalar_of_ty ctx (t : Ast.ty) : scalar =
  match base_ty t with
  | Ast.TPrim ("bool", _) -> SBool
  | Ast.TPrim ("string", _) -> SString
  | Ast.TPrim (p, _) when List.mem p int_prims -> SInt
  | Ast.TName (n, [], _) -> (
      match decl_by_name ctx n with
      | Some { dkind = Ast.DEnum { cases }; _ } ->
          SEnum (List.map (fun (c : Ast.enum_case) -> c.Ast.cname) cases)
      | _ -> SOther)
  | _ -> SOther

(* ── Reference collection ──────────────────────────────────────────────── *)

(* Field references inside one template string, e.g. "A{.x}B{.y.z}". Input
   placeholders ({id}) are not field references and are skipped here. *)
let template_refs (s : string) : string list list =
  let d = ref [] in
  let dpos : Span.pos = { line = 0; col = 0; offset = 0 } in
  let dspan : Span.span = { start = dpos; finish = dpos } in
  let parts = Lower.parse_template ~diags:d ~span:dspan s in
  List.filter_map (function Ir.Tpl_field p -> Some p | _ -> None) parts

(* The field references a member's trait metadata consumes (env refs, format
   placeholders, bind sources), paired with the span to report against. Match
   refs are excluded: [check_match] owns their reporting with better messages;
   [member_refs] below folds them back in for dependency analysis. *)
let member_trait_refs (m : Ast.member) : (string list * Span.span) list =
  let of_trait (tr : Ast.trait) =
    match tr.Ast.tname with
    | "env" ->
        List.filter_map
          (function Ast.ARef r -> Some (r.Ast.segs, tr.tspan) | _ -> None)
          tr.targs
    | "format" ->
        List.concat_map
          (function
            | Ast.AString s ->
                List.map (fun p -> (p, tr.Ast.tspan)) (template_refs s)
            | _ -> [])
          tr.targs
    | "bind" ->
        List.filter_map
          (function Ast.ARef r -> Some (r.Ast.segs, tr.tspan) | _ -> None)
          tr.targs
    | _ -> []
  in
  List.concat_map of_trait m.mtraits

(* Every field reference a member consumes, including the match's subject and
   arms; the dependency graph and the lazy-resolution walk read this. *)
let member_refs (m : Ast.member) : (string list * Span.span) list =
  let of_match (fm : Ast.field_match) =
    (fm.subject.segs, fm.subject.ref_span)
    :: List.concat_map
         (fun (a : Ast.match_arm) ->
           match a.value with
           | Ast.AVRef r -> [ (r.segs, r.ref_span) ]
           | Ast.AVSources traits ->
               List.concat_map
                 (fun (tr : Ast.trait) ->
                   List.filter_map
                     (function
                       | Ast.ARef r -> Some (r.Ast.segs, tr.Ast.tspan)
                       | _ -> None)
                     tr.Ast.targs)
                 traits
           | _ -> [])
         fm.arms
  in
  member_trait_refs m
  @ match m.mmatch with Some fm -> of_match fm | None -> []

let protocol_trait_names = [ "header"; "timeout"; "retry" ]

(* The field references an operation's protocol traits consume: the @http
   endpoint and path template, @header keys and values, @timeout and @retry. *)
let op_refs (op : Ast.decl) : (string list * Span.span) list =
  List.concat_map
    (fun (tr : Ast.trait) ->
      if
        String.equal tr.Ast.tname "http"
        || List.mem tr.Ast.tname protocol_trait_names
      then
        List.concat_map
          (fun arg ->
            let rec refs_of = function
              | Ast.ARef r -> [ (r.Ast.segs, tr.Ast.tspan) ]
              | Ast.AString s ->
                  List.map (fun p -> (p, tr.Ast.tspan)) (template_refs s)
              | Ast.AKv (_, v) -> refs_of v
              | _ -> []
            in
            refs_of arg)
          tr.targs
      else [])
    op.Ast.dtraits

(* ── Field-local rules ─────────────────────────────────────────────────── *)

(* Whether a field declares any way to get a value on its own: sources, a
   match, a format derivation, or being a composed config. *)
let has_own_source ctx (m : Ast.member) : bool =
  source_traits m <> []
  || Option.is_some m.mmatch
  || Option.is_some (find_trait "format" m.mtraits)
  ||
  match base_ty m.mtype with
  | Ast.TName (n, [], _) -> role_of_name ctx n = Roles.Config
  | _ -> false

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
      when List.length sources > 1 || Option.is_some m.mmatch || has "format" ->
        [
          dead tr.tspan
            "@arg is explicit and exclusive: the other sources (or match / \
             @format) on this field can never fire";
        ]
    | _ -> []
  in
  let match_diags =
    match m.mmatch with
    | Some fm when (not (has "arg")) && (sources <> [] || has "format") ->
        [
          dead fm.match_span
            "a match is the field's only value; declare sources on the fields \
             its arms reference instead";
        ]
    | _ -> []
  in
  let format_diags =
    match find_trait "format" m.mtraits with
    | Some tr when (not (has "arg")) && Option.is_none m.mmatch && sources <> []
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

(* ── Match validation ──────────────────────────────────────────────────── *)

let pattern_key : Ast.match_pattern -> string = function
  | Ast.PString s -> "s:" ^ s
  | Ast.PInt n -> "i:" ^ string_of_int n
  | Ast.PName n -> "n:" ^ n
  | Ast.PWildcard -> "_"

let check_match ctx (fields : Ast.member list) (m : Ast.member)
    (fm : Ast.field_match) : Diagnostic.t list =
  match resolve_path ctx fields fm.subject.segs with
  | None ->
      [
        err Error_codes.field_ref_unknown fm.subject.ref_span
          "unknown field '%s' as the match subject" (path_str fm.subject.segs);
      ]
  | Some subject -> (
      match scalar_of_ty ctx subject.mtype with
      | SOther ->
          [
            err Error_codes.match_invalid fm.subject.ref_span
              "match subject '%s' must be a bool, string, integer, or enum \
               field"
              (path_str fm.subject.segs);
          ]
      | scalar ->
          let pattern_diags (a : Ast.match_arm) =
            match (scalar, a.pat) with
            | _, Ast.PWildcard -> []
            | SBool, Ast.PName ("true" | "false") -> []
            | SBool, _ ->
                [
                  err Error_codes.match_invalid a.pat_span
                    "a bool match takes the patterns true, false, or '_'";
                ]
            | SString, Ast.PString _ -> []
            | SString, _ ->
                [
                  err Error_codes.match_invalid a.pat_span
                    "a string match takes quoted literal patterns or '_'";
                ]
            | SInt, Ast.PInt _ -> []
            | SInt, _ ->
                [
                  err Error_codes.match_invalid a.pat_span
                    "an integer match takes integer literal patterns or '_'";
                ]
            | SEnum cases, Ast.PName n when List.mem n cases -> []
            | SEnum _, Ast.PName n ->
                [
                  err Error_codes.match_invalid a.pat_span
                    "'%s' is not a case of the matched enum" n;
                ]
            | SEnum _, _ ->
                [
                  err Error_codes.match_invalid a.pat_span
                    "an enum match takes bare case names or '_'";
                ]
            | SOther, _ -> []
          in
          let value_diags (a : Ast.match_arm) =
            match a.value with
            | Ast.AVRef r -> (
                match resolve_path ctx fields r.segs with
                | Some _ -> []
                | None ->
                    [
                      err Error_codes.field_ref_unknown r.ref_span
                        "unknown field '%s' in a match arm" (path_str r.segs);
                    ])
            | Ast.AVSources traits ->
                List.concat_map
                  (fun (tr : Ast.trait) ->
                    if List.mem tr.Ast.tname [ "env"; "default" ] then
                      List.filter_map
                        (function
                          | Ast.ARef r -> (
                              match resolve_path ctx fields r.Ast.segs with
                              | Some _ -> None
                              | None ->
                                  Some
                                    (err Error_codes.field_ref_unknown tr.tspan
                                       "unknown field '%s' in a match arm \
                                        source"
                                       (path_str r.segs)))
                          | _ -> None)
                        tr.targs
                    else
                      [
                        err Error_codes.match_invalid tr.tspan
                          "a match arm only stacks @env/@default sources";
                      ])
                  traits
            | Ast.AVString _ | Ast.AVInt _ | Ast.AVName _ -> []
          in
          let dup_and_reach_diags =
            let rec go seen wild = function
              | [] -> []
              | (a : Ast.match_arm) :: rest ->
                  let here =
                    if wild then
                      [
                        err Error_codes.match_invalid a.pat_span
                          "unreachable arm: it follows the '_' wildcard";
                      ]
                    else if List.mem (pattern_key a.pat) seen then
                      [
                        err Error_codes.match_invalid a.pat_span
                          "duplicate pattern in match";
                      ]
                    else []
                  in
                  here
                  @ go
                      (pattern_key a.pat :: seen)
                      (wild || a.pat = Ast.PWildcard)
                      rest
            in
            go [] false fm.arms
          in
          let exhaustive =
            List.exists
              (fun (a : Ast.match_arm) -> a.pat = Ast.PWildcard)
              fm.arms
            ||
            let covered =
              List.filter_map
                (fun (a : Ast.match_arm) ->
                  match a.pat with Ast.PName n -> Some n | _ -> None)
                fm.arms
            in
            match scalar with
            | SBool -> List.mem "true" covered && List.mem "false" covered
            | SEnum cases -> List.for_all (fun c -> List.mem c covered) cases
            | _ -> false
          in
          let exhaustive_diags =
            if exhaustive then []
            else
              [
                err Error_codes.match_not_exhaustive fm.match_span
                  "match on field '%s' is not exhaustive; add the missing \
                   cases or a '_' arm"
                  m.mname;
              ]
          in
          List.concat_map pattern_diags fm.arms
          @ List.concat_map value_diags fm.arms
          @ dup_and_reach_diags @ exhaustive_diags)

(* ── Resolution graph: reachability, cycles, lazy chains ───────────────── *)

(* Dependency heads of a field, restricted to refs that resolve (unresolvable
   ones are reported separately as TC0038). *)
let dep_heads ctx (fields : Ast.member list) (m : Ast.member) : string list =
  List.filter_map
    (fun (segs, _) ->
      match segs with
      | head :: _ when Option.is_some (resolve_path ctx fields [ head ]) ->
          Some head
      | _ -> None)
    (member_refs m)

let check_cycles ctx (fields : Ast.member list) : Diagnostic.t list =
  let field name =
    List.find_opt (fun (m : Ast.member) -> String.equal m.mname name) fields
  in
  let deps name =
    match field name with Some m -> dep_heads ctx fields m | None -> []
  in
  (* Iterative DFS with an explicit color map; a back edge closes a cycle. *)
  let color = Hashtbl.create 16 in
  let diags = ref [] in
  let rec visit stack name =
    match Hashtbl.find_opt color name with
    | Some `Done -> ()
    | Some `Active ->
        let cycle =
          let rec take acc = function
            | [] -> List.rev acc
            | x :: _ when String.equal x name -> List.rev (x :: acc)
            | x :: rest -> take (x :: acc) rest
          in
          take [] stack
        in
        let span =
          match field name with
          | Some m -> m.mname_span
          | None -> (List.hd fields).mname_span
        in
        diags :=
          err Error_codes.resolution_cycle span
            "field resolution forms a cycle: %s"
            (String.concat " -> " (List.rev (name :: cycle)))
          :: !diags
    | None ->
        Hashtbl.replace color name `Active;
        List.iter (visit (name :: stack)) (deps name);
        Hashtbl.replace color name `Done
  in
  List.iter (fun (m : Ast.member) -> visit [] m.mname) fields;
  List.rev !diags

(* [broken name] is the chain from [name] down to the first dependency that
   declares no source at all, or [None] when the field statically resolves.
   Cycles are reported separately, so an active node cuts off as resolvable. *)
let broken ctx (fields : Ast.member list) : string -> string list option =
  let memo = Hashtbl.create 16 in
  let rec go active name =
    if List.mem name active then None
    else
      match Hashtbl.find_opt memo name with
      | Some r -> r
      | None ->
          let r =
            match
              List.find_opt
                (fun (m : Ast.member) -> String.equal m.mname name)
                fields
            with
            | None -> None
            | Some m ->
                if not (has_own_source ctx m) then Some [ name ]
                else
                  List.find_map
                    (fun dep ->
                      Option.map
                        (fun chain -> name :: chain)
                        (go (name :: active) dep))
                    (dep_heads ctx fields m)
          in
          Hashtbl.replace memo name r;
          r
  in
  go []

let chain_error span (path : string list) (chain : string list) =
  let last = List.nth chain (List.length chain - 1) in
  err Error_codes.field_unresolvable span
    "cannot resolve '%s': %s ('%s' declares no value source)" (path_str path)
    (String.concat " <- " chain)
    last

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
            | [ Ast.AName target; Ast.ARef r ] ->
                (if
                   List.exists
                     (fun (cm : Ast.member) -> String.equal cm.mname target)
                     config_fields
                 then []
                 else
                   [
                     err Error_codes.bind_invalid tr.tspan
                       "'%s' is not a field of the composed config '%s'" target
                       n;
                   ])
                @
                if Option.is_some (resolve_path ctx fields r.segs) then []
                else
                  [
                    err Error_codes.field_ref_unknown tr.tspan
                      "unknown field '%s' as a @bind source" (path_str r.segs);
                  ]
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

(* ── Operations ────────────────────────────────────────────────────────── *)

(* Protocol checks shared by every op: @header/@timeout/@retry require @http
   (a purely local operation has no protocol surface). *)
let check_protocol_positions (op : Ast.decl) : Diagnostic.t list =
  if Option.is_some (find_trait "http" op.dtraits) then []
  else
    List.filter_map
      (fun (tr : Ast.trait) ->
        if List.mem tr.Ast.tname protocol_trait_names then
          Some
            (err Error_codes.protocol_trait_invalid tr.tspan
               "@%s is a protocol trait and requires @http on the operation"
               tr.tname)
        else None)
      op.Ast.dtraits

(* A loose (non-entry) operation has no field scope: any field reference in a
   protocol trait, and any endpoint:, belongs to an entry. *)
let check_loose_op (op : Ast.decl) : Diagnostic.t list =
  let ref_diags =
    List.map
      (fun ((segs : string list), span) ->
        err Error_codes.protocol_trait_invalid span
          "field reference '%s' is only available on an operation declared in \
           an entry body"
          (path_str segs))
      (op_refs op)
  in
  let endpoint_diags =
    match find_trait "http" op.dtraits with
    | Some tr when Option.is_some (kv_arg "endpoint" tr.targs) -> (
        match kv_arg "endpoint" tr.targs with
        | Some (Ast.ARef _) -> [] (* already reported as a field reference *)
        | _ ->
            [
              err Error_codes.protocol_trait_invalid tr.tspan
                "endpoint: is only available on an operation declared in an \
                 entry body";
            ])
    | _ -> []
  in
  check_protocol_positions op @ ref_diags @ endpoint_diags

let check_entry_op ctx (fields : Ast.member list) (op : Ast.decl) :
    Diagnostic.t list =
  let resolve segs = resolve_path ctx fields segs in
  let ref_diags =
    List.filter_map
      (fun (segs, span) ->
        if Option.is_some (resolve segs) then None
        else
          Some
            (err Error_codes.field_ref_unknown span
               "unknown field '%s' referenced by an operation trait"
               (path_str segs)))
      (op_refs op)
  in
  let http_diags =
    match find_trait "http" op.dtraits with
    | None -> []
    | Some http -> (
        match kv_arg "endpoint" http.targs with
        | None ->
            [
              err Error_codes.entry_endpoint_missing http.tspan
                "an entry operation's @http must name its endpoint, e.g. \
                 @http(..., endpoint: .endpoint)";
            ]
        | Some (Ast.ARef r) -> (
            match resolve r.segs with
            | Some m when scalar_of_ty ctx m.mtype = SString -> []
            | Some _ ->
                [
                  err Error_codes.entry_endpoint_missing r.ref_span
                    "endpoint '%s' must reference a string field"
                    (path_str r.segs);
                ]
            | None -> [] (* unknown ref already reported above *))
        | Some _ ->
            [
              err Error_codes.entry_endpoint_missing http.tspan
                "endpoint: takes a field reference (.field), not a literal";
            ])
  in
  let typed_ref name want scalar_desc =
    List.concat_map
      (fun (tr : Ast.trait) ->
        match tr.Ast.targs with
        | [ Ast.ARef r ] -> (
            match resolve r.segs with
            | None -> [] (* already reported *)
            | Some m ->
                if want ctx m.mtype then []
                else
                  [
                    err Error_codes.protocol_trait_invalid r.ref_span
                      "@%s must reference a %s field" name scalar_desc;
                  ])
        | _ ->
            [
              err Error_codes.protocol_trait_invalid tr.tspan
                "@%s takes a single field reference, e.g. @%s(.field)" name name;
            ])
      (traits_named name op.dtraits)
  in
  let timeout_diags =
    typed_ref "timeout"
      (fun _ t ->
        match base_ty t with Ast.TPrim ("duration", _) -> true | _ -> false)
      "duration"
  in
  let retry_diags =
    typed_ref "retry" (fun ctx t -> scalar_of_ty ctx t = SInt) "integer"
  in
  let header_diags =
    List.concat_map
      (fun (tr : Ast.trait) ->
        match tr.Ast.targs with
        | [ key; _value ] -> (
            match key with
            | Ast.AString _ -> []
            | _ ->
                [
                  err Error_codes.protocol_trait_invalid tr.tspan
                    "@header expects a string key (a literal, possibly with \
                     {.field} placeholders)";
                ])
        | _ ->
            [
              err Error_codes.protocol_trait_invalid tr.tspan
                "@header expects a key and a value, e.g. @header(\"X-Name\", \
                 .field)";
            ])
      (traits_named "header" op.dtraits)
  in
  check_protocol_positions op
  @ ref_diags @ http_diags @ timeout_diags @ retry_diags @ header_diags

(* ── Wire boundary ─────────────────────────────────────────────────────── *)

(* Any type reference to an entry/config from a wire position: op inputs and
   outputs, declared errors, wire struct members, union payloads. *)
let rec boundary_ty ctx (t : Ast.ty) : Diagnostic.t list =
  match t with
  | Ast.TName (n, args, span) ->
      (match role_of_name ctx n with
        | Roles.Entry | Roles.Config ->
            [
              err Error_codes.entry_wire_boundary span
                "'%s' is an %s and never crosses the wire" n
                (match role_of_name ctx n with
                | Roles.Entry -> "entry"
                | _ -> "config");
            ]
        | Roles.Wire -> [])
      @ List.concat_map (boundary_ty ctx) args
  | Ast.TQName (_, _, args, _) -> List.concat_map (boundary_ty ctx) args
  | Ast.TList (t, _) | Ast.TNullable (t, _) -> boundary_ty ctx t
  | Ast.TMap (k, v, _) -> boundary_ty ctx k @ boundary_ty ctx v
  | Ast.TPrim _ | Ast.TError _ -> []

let check_op_boundary ctx (op : Ast.decl) : Diagnostic.t list =
  match op.dkind with
  | Ast.DOp { input; output } ->
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

(* Wire members must not compose entries/configs; entry fields must not name
   an entry. A config-typed entry field is the composition point (fine); a
   config/entry-typed anything else crosses a closed boundary. *)
let check_member_boundary ctx ~(container : Roles.role) (m : Ast.member) :
    Diagnostic.t list =
  match base_ty m.mtype with
  | Ast.TName (n, _, span) -> (
      match (role_of_name ctx n, container) with
      | Roles.Entry, _ ->
          [
            err Error_codes.entry_wire_boundary span
              "'%s' is an entry and cannot be a field type" n;
          ]
      | Roles.Config, Roles.Entry -> []
      | Roles.Config, _ ->
          [
            err Error_codes.entry_wire_boundary span
              "'%s' is a config and can only be composed by an entry field" n;
          ]
      | Roles.Wire, _ -> [])
  | _ -> []

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
        @ check_transforms m @ check_nullable_field m
        @ check_binds ctx members m
        @ check_member_boundary ctx ~container:Roles.Entry m
        @ unresolved_ref_diags
        @
        match m.mmatch with
        | Some fm -> check_match ctx members m fm
        | None -> [])
      members
  in
  let cycle_diags = check_cycles ctx members in
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
    List.filter_map
      (fun ((segs : string list), span) ->
        match segs with
        | head :: _ when Option.is_some (resolve_path ctx members segs) ->
            Option.map (chain_error span segs) (broken_of head)
        | _ -> None)
      consumptions
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
  @ field_diags @ cycle_diags @ op_diags @ lazy_diags @ sourceless_diags

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
    ctx.decls

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
        @ check_transforms m @ check_nullable_field m
        @ check_member_boundary ctx ~container:Roles.Config m
        @ (traits_named "bind" m.mtraits
          |> List.map (fun (tr : Ast.trait) ->
              err Error_codes.bind_invalid tr.Ast.tspan
                "@bind only lives at a composition point: an entry field whose \
                 type is a config"))
        @ unresolved_ref_diags @ sourceless
        @
        match m.mmatch with
        | Some fm -> check_match ctx members m fm
        | None -> [])
      members
  in
  check_generics d params "config" @ check_cycles ctx members @ field_diags

(* ── Module pass ───────────────────────────────────────────────────────── *)

let check_decls (decls : Ast.decl list) : Diagnostic.t list =
  let ctx = { decls; roles = Roles.classify decls } in
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
                (check_member_boundary ctx ~container:Roles.Wire)
                members)
      | Ast.DUnion { variants; _ } ->
          check_non_struct_sources d
          @ List.concat_map
              (fun (v : Ast.union_variant) ->
                match v.vpayload with Some t -> boundary_ty ctx t | None -> [])
              variants
      | Ast.DEnum _ -> check_non_struct_sources d
      | Ast.DOp _ -> check_op_boundary ctx d @ check_loose_op d
      | Ast.DExt _ -> [])
    decls
