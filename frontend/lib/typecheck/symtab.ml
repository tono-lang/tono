(* Symbol table over one module's declarations. Cross-module resolution builds on
   top of this per-module table (see [Modules]); within a module every shape
   lives in one flat namespace. *)

module SMap = Map.Make (String)

type entry = { arity : int; pub : bool; decl_span : Span.span }
type t = entry SMap.t

let arity_of (d : Ast.decl) : int =
  match d.dkind with
  | Ast.DStruct { params; _ } | Ast.DUnion { params; _ } -> List.length params
  | Ast.DEnum _ | Ast.DOp _ | Ast.DExt _ -> 0

(* Build the table, reporting a duplicate-shape diagnostic (pointing at the
   redefinition, naming the first definition) for any repeated name. The first
   definition wins so later lookups resolve to it. *)
let build (decls : Ast.decl list) : t * Diagnostic.t list =
  List.fold_left
    (fun (tbl, diags) (d : Ast.decl) ->
      (* Extensions live in their own namespace (Check_ext validates them); they
         are not shapes, so they never enter the shape table or collide with one. *)
      match d.dkind with
      | Ast.DExt _ -> (tbl, diags)
      | _ -> (
          match SMap.find_opt d.dname tbl with
          | Some prev ->
              let msg =
                Printf.sprintf "duplicate shape '%s' (first defined at %s)"
                  d.dname
                  (Span.to_string prev.decl_span)
              in
              ( tbl,
                Diagnostic.error ~code:Error_codes.duplicate_shape d.dname_span
                  msg
                :: diags )
          | None ->
              ( SMap.add d.dname
                  { arity = arity_of d; pub = d.pub; decl_span = d.dname_span }
                  tbl,
                diags )))
    (SMap.empty, []) decls

let find name (t : t) = SMap.find_opt name t
