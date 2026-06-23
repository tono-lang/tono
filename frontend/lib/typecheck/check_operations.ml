(* Operation error references. Each @errors entry on an operation names an error
   shape the operation may return and must resolve to a declared shape (TC0014).
   Inputs and outputs are ordinary type references, resolved like any other in
   [Resolve], so only the error list is checked here. Non-name @errors arguments
   are rejected during lowering and never reach this list. *)

let err code span fmt = Printf.ksprintf (Diagnostic.error ~code span) fmt

let check_decl (tbl : Symtab.t) (d : Ast.decl) : Diagnostic.t list =
  match d.dkind with
  | Ast.DOp _ ->
      List.concat_map
        (fun (tr : Ast.trait) ->
          if String.equal tr.tname "errors" then
            List.filter_map
              (function
                | Ast.AName name when Option.is_none (Symtab.find name tbl) ->
                    Some
                      (err Error_codes.unresolved_operation_ref tr.tspan
                         "unresolved error type '%s'" name)
                | _ -> None)
              tr.targs
          else [])
        d.dtraits
  | Ast.DStruct _ | Ast.DEnum _ | Ast.DUnion _ -> []

let check_decls (tbl : Symtab.t) (file : Ast.file) : Diagnostic.t list =
  List.concat_map (check_decl tbl) file
