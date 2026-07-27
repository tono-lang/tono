(* The implementation count for operations. An operation is a contract with no
   body: something has to supply the body, and exactly one thing may. Today that
   is either a protocol binding (@http, resolved into a wire descriptor) or a
   bespoke implementation bound per language by "ext impl".

   The rules this pass owns, all of which need source spans the IR has dropped:

   - an "ext impl" names the operation it implements, so the name must reach an
     operation declared in this module (TC0048). Extensions are per-module, like
     hooks, so an impl and its operation live in the same file;
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
   the backend knows which targets are being emitted. *)

let err code span fmt = Printf.ksprintf (Diagnostic.error ~code span) fmt

let find_trait name (traits : Ast.trait list) : Ast.trait option =
  List.find_opt (fun (t : Ast.trait) -> String.equal t.Ast.tname name) traits

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
        | [ s ] ->
            Hashtbl.add bound (site_key s) d;
            []
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
