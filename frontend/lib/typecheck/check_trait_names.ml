(* Traits nothing will read (TC0054). A misspelled or invented bare trait used
   to pass in silence: no checker matches its name, so it falls through to the
   trait bag, reaches the IR, and lands in the generated SDK doing nothing. The
   value that would have been carried is dropped just as quietly.

   A warning, not an error: the IR's trait vocabulary is open (an id and an
   arbitrary JSON value, carried through unchanged), so a trait outside the
   compiler's vocabulary is not malformed, only inert. Existing specs keep
   compiling while the typo surfaces. *)

let warn code span fmt = Printf.ksprintf (Diagnostic.warning ~code span) fmt

let unknown (traits : Ast.trait list) : Diagnostic.t list =
  List.filter_map
    (fun (tr : Ast.trait) ->
      (* A namespaced trait belongs to a catalog with its own validation
         (TC0045), which reports a bad one with the catalog in hand. *)
      if Trait_vocab.is_namespaced tr.Ast.tname || Trait_vocab.is_known tr.tname
      then None
      else
        let hint =
          match Trait_vocab.nearest tr.tname with
          | Some near -> Printf.sprintf "; did you mean @%s?" near
          | None -> ""
        in
        Some
          (warn Error_codes.unknown_trait tr.tspan
             "unknown trait @%s: nothing reads it, so it is carried into the \
              IR and generates nothing%s"
             tr.tname hint))
    traits

(* Every position a trait can occupy, matching the traversal the repeat check
   already walks. *)
let rec check_decl (d : Ast.decl) : Diagnostic.t list =
  match d.dkind with
  | Ast.DOp _ -> unknown d.dtraits
  | Ast.DStruct { members; ops; _ } ->
      unknown d.dtraits
      @ List.concat_map (fun (m : Ast.member) -> unknown m.Ast.mtraits) members
      @ List.concat_map check_decl ops
  | Ast.DUnion { variants; _ } ->
      unknown d.dtraits
      @ List.concat_map
          (fun (v : Ast.union_variant) -> unknown v.Ast.vtraits)
          variants
  | Ast.DEnum { cases } ->
      unknown d.dtraits
      @ List.concat_map (fun (c : Ast.enum_case) -> unknown c.Ast.ctraits) cases
  | Ast.DExt _ -> unknown d.dtraits
  | Ast.DTest _ -> []

let check_decls (decls : Ast.decl list) : Diagnostic.t list =
  List.concat_map check_decl decls
