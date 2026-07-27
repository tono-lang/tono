(* The implementation count for operations. An operation is a contract with no
   body: something has to supply the body, and exactly one thing may. Today that
   is either a protocol binding (@http, resolved into a wire descriptor) or a
   bespoke implementation bound per language by "ext impl".

   The rules this pass owns, all of which need source spans the IR has dropped:

   - an "ext impl" names the operation it implements, so the name must reach an
     operation an entry declares in this module (TC0048). Extensions are
     per-module, like hooks, so an impl and its operation live in the same file;
     and only an entry operation gets a generated body, so an impl on a loose one
     would be declared and then silently never called;
   - a bare operation name must reach exactly one operation: two entries in one
     module may declare the same operation name, and the qualified "entry.op"
     form resolves that (TC0049);
   - an operation is implemented at most once, so @http plus an impl, or two
     impls reaching the same operation, is a conflict (TC0050);
   - an entry operation is implemented at least once: with neither @http nor an
     impl the generated client would have a method that cannot run (TC0051).
     A loose operation carries no such requirement (it is a bare contract that
     entries and tooling reference, and the generators already skip it).

   Whether every generated language has a binding is the generator's gate: only
   the backend knows which targets are being emitted.

   One rule is about the raw form rather than the count: a raw implementation
   reports a failure by its code alone, having no protocol status to match on, so
   a declared error with no @errorCode has nothing that could select it and would
   silently resolve to the generic fallback (TC0053). *)

let err code span fmt = Printf.ksprintf (Diagnostic.error ~code span) fmt
let warn code span fmt = Printf.ksprintf (Diagnostic.warning ~code span) fmt

let find_trait name (traits : Ast.trait list) : Ast.trait option =
  List.find_opt (fun (t : Ast.trait) -> String.equal t.Ast.tname name) traits

(* The shapes an operation names in its @errors traits, in declaration order. *)
let declared_error_names (op : Ast.decl) : string list =
  List.concat_map
    (fun (tr : Ast.trait) ->
      if String.equal tr.Ast.tname "errors" then
        List.filter_map (function Ast.AName n -> Some n | _ -> None) tr.targs
      else [])
    op.Ast.dtraits

(* One operation declaration and, when it lives in an entry body, the entry that
   holds it. *)
type op_site = { op : Ast.decl; entry : string option }

let op_sites (decls : Ast.decl list) : op_site list =
  List.concat_map
    (fun (d : Ast.decl) ->
      match d.dkind with
      | Ast.DOp _ -> [ { op = d; entry = None } ]
      | Ast.DStruct { ops; _ } ->
          List.map (fun (o : Ast.decl) -> { op = o; entry = Some d.dname }) ops
      | _ -> [])
    decls

(* The site's identity, and the name a diagnostic should show: the qualified form
   for an entry operation, so two same-named operations are told apart. *)
let site_key (s : op_site) : string =
  match s.entry with
  | None -> s.op.Ast.dname
  | Some e -> e ^ "." ^ s.op.Ast.dname

(* The names an impl may use to reach this site. An entry operation answers to
   both its bare name and the qualified one. *)
let site_names (s : op_site) : string list =
  match s.entry with
  | None -> [ s.op.Ast.dname ]
  | Some _ -> [ s.op.Ast.dname; site_key s ]

let impl_decls (decls : Ast.decl list) : Ast.decl list =
  List.filter
    (fun (d : Ast.decl) ->
      match d.dkind with
      | Ast.DExt { ekind = Ast.EImpl; _ } -> true
      | _ -> false)
    decls

(* A raw implementation discriminates a failure by its code, so an error the
   operation declares without an @errorCode can never be selected: the glue
   resolves it to the generic fallback instead. Reported at the impl, which is
   what makes the errors unreachable. *)
let unreachable_raw_errors (decls : Ast.decl list) (impl : Ast.decl)
    (site : op_site) : Diagnostic.t list =
  let is_raw =
    match impl.Ast.dkind with
    | Ast.DExt { eraw = Some _; _ } -> true
    | _ -> false
  in
  if not is_raw then []
  else
    List.filter_map
      (fun name ->
        match
          List.find_opt
            (fun (d : Ast.decl) -> String.equal d.Ast.dname name)
            decls
        with
        | Some target when find_trait "errorCode" target.Ast.dtraits = None ->
            Some
              (warn Error_codes.raw_error_unreachable impl.Ast.dname_span
                 "'%s' declares the error '%s', which has no @errorCode; a raw \
                  implementation matches on the code alone, so this error can \
                  only ever surface as the generic failure"
                 (site_key site) name)
        | _ -> None)
      (declared_error_names site.op)

let check_decls (decls : Ast.decl list) : Diagnostic.t list =
  let sites = op_sites decls in
  (* site key -> every impl that reached it, so the conflict rule can name the
     second one. *)
  let bound : (string, Ast.decl) Hashtbl.t = Hashtbl.create 8 in
  let impl_diags =
    List.concat_map
      (fun (d : Ast.decl) ->
        match
          List.filter (fun s -> List.mem d.Ast.dname (site_names s)) sites
        with
        | [ s ] when s.entry <> None ->
            Hashtbl.add bound (site_key s) d;
            unreachable_raw_errors decls d s
        | [ _ ] ->
            [
              err Error_codes.ext_impl_unknown_op d.dname_span
                "'%s' is a loose operation, which no client implements; move \
                 it into the entry that should expose it"
                d.dname;
            ]
        | [] ->
            [
              err Error_codes.ext_impl_unknown_op d.dname_span
                "'ext impl %s' names no operation in this module; an impl and \
                 the operation it implements are declared together"
                d.dname;
            ]
        | matches ->
            [
              err Error_codes.ext_impl_ambiguous_op d.dname_span
                "'%s' names %d operations; write the entry to pick one (%s)"
                d.dname (List.length matches)
                (String.concat ", " (List.map site_key matches));
            ])
      (impl_decls decls)
  in
  let count_diags =
    List.concat_map
      (fun s ->
        (* [find_all] yields the most recent first; source order reads better. *)
        let bound_here = List.rev (Hashtbl.find_all bound (site_key s)) in
        match (find_trait "http" s.op.Ast.dtraits, bound_here) with
        | Some _, impl :: _ ->
            [
              err Error_codes.op_implementation_conflict impl.Ast.dname_span
                "operation '%s' is already bound to a protocol by @http; an \
                 operation is implemented exactly once"
                (site_key s);
            ]
        | None, _ :: second :: _ ->
            [
              err Error_codes.op_implementation_conflict second.Ast.dname_span
                "operation '%s' already has an impl; an operation is \
                 implemented exactly once"
                (site_key s);
            ]
        | None, [] when s.entry <> None ->
            [
              err Error_codes.op_implementation_missing s.op.Ast.dname_span
                "entry operation '%s' has no implementation; bind it to a \
                 protocol with @http or to bespoke sources with 'ext impl %s'"
                (site_key s) s.op.Ast.dname;
            ]
        | _ -> [])
      sites
  in
  impl_diags @ count_diags
