(* The scope an entry (or config) resolves against: role lookups, field-path
   resolution (including descent into struct-typed fields), scalar
   classification for match subjects, collection of the field references that
   traits and operations consume, and the resolution graph over those
   references (dependency heads, cycle detection, and the lazy chain walk that
   names the first field with no declared source). *)

let err code span fmt = Printf.ksprintf (Diagnostic.error ~code span) fmt

let find_trait name (traits : Ast.trait list) : Ast.trait option =
  List.find_opt (fun (t : Ast.trait) -> String.equal t.tname name) traits

(* [Trait_vocab.sources] plus [default]: a member with no @arg/@with/@env
   still resolves from its default, so the default counts as a source here
   even though it lives in [Trait_vocab.members] (it also governs plain wire
   members, which have no scope to resolve against). *)
let source_names = Trait_vocab.sources @ [ "default" ]

let source_traits (m : Ast.member) : Ast.trait list =
  List.filter
    (fun (t : Ast.trait) -> List.mem t.Ast.tname source_names)
    m.Ast.mtraits

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

(* ── Op-parameter resolution ──────────────────────────────────────────────
   An op's named parameter lives in a separate namespace from entry fields
   (the AST has no member list for it, just a name and a type), so it gets
   its own resolver rather than reusing [resolve_path]'s [Ast.member list].
   Codegen only ever renders a parameter reference zero or one segment deep
   (the whole value, or one member of a struct-typed parameter — an op has
   exactly one parameter, so "member of the parameter" is the same decoded
   record the legacy [{name}] placeholder already reads), so resolution caps
   there too: a deeper path is reported unresolved rather than silently
   accepted and left unrenderable. *)
type param_ref = Whole of Ast.ty | Member of Ast.member

let resolve_param ctx (pname : string option) (pty : Ast.ty option)
    (segs : string list) : param_ref option =
  match (pname, pty, segs) with
  | Some p, Some ty, [ head ] when String.equal p head -> Some (Whole ty)
  | Some p, Some ty, [ head; member ] when String.equal p head -> (
      match base_ty ty with
      | Ast.TName (n, [], _) -> (
          match struct_members ctx n with
          | Some ms ->
              Option.map
                (fun m -> Member m)
                (List.find_opt
                   (fun (m : Ast.member) -> String.equal m.mname member)
                   ms)
          | None -> None)
      | _ -> None)
  | _ -> None

(* [.name] resolves against what the op declares: its own parameter first
   (an op-param name shadows a same-named entry field, ordinary lexical
   shadowing), then the entry's fields. *)
type resolved_ref = RField of Ast.member | RParam of param_ref

let resolve_ref ctx (fields : Ast.member list) ~(pname : string option)
    ~(pty : Ast.ty option) (segs : string list) : resolved_ref option =
  match resolve_param ctx pname pty segs with
  | Some p -> Some (RParam p)
  | None -> Option.map (fun m -> RField m) (resolve_path ctx fields segs)

let op_param (op : Ast.decl) : string option * Ast.ty option =
  match op.Ast.dkind with
  | Ast.DOp { pname; input; _ } -> (pname, input)
  | _ -> (None, None)

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
  let parts = Template.parse ~diags:d ~span:dspan s in
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

(* The field references inside an extern call's own arguments (its
   [call_arg]s), recursing into a ctor argument's fields — which carry the
   richer [trait_arg] grammar (the same one @body's mapper and other trait
   arguments use), so a nested list/ctor/call inside one of those fields is
   walked too. [parse_trait_value] never produces [Ast.AKv] in this position
   (that form is only reachable at a trait's own top-level "key: value"
   argument, a different grammar position [op_refs] walks instead), so it
   folds into the no-refs catch-all rather than carrying dead recursion. *)
let rec trait_arg_refs (a : Ast.trait_arg) : (string list * Span.span) list =
  match a with
  | Ast.ARef r -> [ (r.Ast.segs, r.Ast.ref_span) ]
  | Ast.AList xs -> List.concat_map trait_arg_refs xs
  | Ast.ACtor c ->
      List.concat_map (fun (_, _, v) -> trait_arg_refs v) c.Ast.ctor_fields
  | Ast.ACall ce -> ce_refs ce
  | Ast.AString _ | Ast.AInt _ | Ast.AFloat _ | Ast.AName _ | Ast.AKv _ -> []

and call_args_refs (args : Ast.call_arg list) : (string list * Span.span) list =
  List.concat_map
    (function
      | Ast.CaRef r -> [ (r.Ast.segs, r.Ast.ref_span) ]
      | Ast.CaCtor c ->
          List.concat_map (fun (_, _, v) -> trait_arg_refs v) c.Ast.ctor_fields
      | Ast.CaParam _ | Ast.CaLit _ -> [])
    args

and ce_refs (ce : Ast.call_expr) : (string list * Span.span) list =
  call_args_refs ce.Ast.ce_args

(* A handle method call reads its receiver (the handle field) and whatever
   its arguments reference: both are dependencies of the field it feeds. *)
let hc_refs (hc : Ast.op_impl) : (string list * Span.span) list =
  (hc.Ast.oi_recv.Ast.segs, hc.Ast.oi_recv.Ast.ref_span)
  :: call_args_refs hc.Ast.oi_args

(* Every field reference a member consumes, including the match's subject and
   arms, an extern call's own argument refs, and a handle method call's
   receiver and argument refs; the dependency graph and the lazy-resolution
   walk read this. *)
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
  @
  match m.mvalue with
  | Some (Ast.MMatch fm) -> of_match fm
  | Some (Ast.MCall ce) -> ce_refs ce
  | Some (Ast.MHandleCall hc) -> hc_refs hc
  | None -> []

let protocol_trait_names = Trait_vocab.protocol

(* The field references an operation's protocol traits consume: the @http
   endpoint and path template, @header/@query keys and values, @body's
   reference (or ctor field references), @timeout and @retry, and an extern
   call's own field-reference arguments (e.g.
   @header("Authorization", companyauth.sign(.request, .secret)) — ".secret"
   is collected here like any other ref; ".request" is the reserved
   canonical-request marker and is excluded, since it never names a field
   to resolve — its position is validated separately by
   [Check_entry_ops.check_request_value]. *)
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
              | Ast.ACtor c ->
                  List.concat_map (fun (_, _, v) -> refs_of v) c.Ast.ctor_fields
              | Ast.ACall ce ->
                  List.filter_map
                    (fun (segs, _) ->
                      if segs = [ "request" ] then None
                      else Some (segs, tr.Ast.tspan))
                    (ce_refs ce)
              | _ -> []
            in
            refs_of arg)
          tr.targs
      else [])
    op.Ast.dtraits

(* Whether a field declares any way to get a value on its own: sources, a
   match, a format derivation, or being a composed config. *)
let has_own_source ctx (m : Ast.member) : bool =
  source_traits m <> []
  || Option.is_some m.mvalue
  || Option.is_some (find_trait "format" m.mtraits)
  ||
  match base_ty m.mtype with
  | Ast.TName (n, [], _) -> role_of_name ctx n = Roles.Config
  | _ -> false

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
   Cycles are reported separately, so an active node cuts off as resolvable;
   a [None] memoized under that cutoff can then hide a chain reachable another
   way, which is sound only because any such graph already carries the cycle
   error. *)
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
