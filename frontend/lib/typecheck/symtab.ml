(* Global symbol table over a single file's declarations. Cross-module resolution
   is a later concern; here every shape lives in one flat namespace. *)

module SMap = Map.Make (String)

type entry = { arity : int; decl_span : Span.span }
type t = entry SMap.t

let arity_of (d : Ast.decl) : int =
  match d.dkind with
  | Ast.DStruct { params; _ } | Ast.DUnion { params; _ } -> List.length params
  | Ast.DEnum _ | Ast.DOp _ -> 0

(* Build the table, reporting a duplicate-shape diagnostic (pointing at the
   redefinition, naming the first definition) for any repeated name. The first
   definition wins so later lookups resolve to it. *)
let build (file : Ast.file) : t * Diagnostic.t list =
  List.fold_left
    (fun (tbl, diags) (d : Ast.decl) ->
      match SMap.find_opt d.dname tbl with
      | Some prev ->
          let msg =
            Printf.sprintf "duplicate shape '%s' (first defined at %s)" d.dname
              (Span.to_string prev.decl_span)
          in
          ( tbl,
            Diagnostic.error ~code:Error_codes.duplicate_shape d.dname_span msg
            :: diags )
      | None ->
          ( SMap.add d.dname { arity = arity_of d; decl_span = d.dname_span } tbl,
            diags ))
    (SMap.empty, []) file

let find name (t : t) = SMap.find_opt name t
