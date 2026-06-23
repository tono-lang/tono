(* Resolve every type reference in a file against the symbol table plus the
   in-scope generic parameters of the enclosing shape. An unresolved name is a
   TC0001. Arity of a generic application is checked in a later phase. *)

let rec resolve_ty ~params ~(tbl : Symtab.t) (ty : Ast.ty) : Diagnostic.t list =
  match ty with
  | Ast.TPrim _ -> [] (* the parser only emits a recognized primitive here *)
  | Ast.TError _ -> [] (* a parse error was already reported *)
  | Ast.TList (t, _) | Ast.TNullable (t, _) -> resolve_ty ~params ~tbl t
  | Ast.TMap (k, v, _) -> resolve_ty ~params ~tbl k @ resolve_ty ~params ~tbl v
  | Ast.TName (name, args, span) ->
      let arg_diags = List.concat_map (resolve_ty ~params ~tbl) args in
      let here =
        if List.mem name params || Symtab.mem name tbl then []
        else
          [
            Diagnostic.error ~code:Error_codes.unknown_type span
              (Printf.sprintf "unknown type '%s'" name);
          ]
      in
      here @ arg_diags

let resolve_decl (tbl : Symtab.t) (d : Ast.decl) : Diagnostic.t list =
  match d.dkind with
  | Ast.DStruct { params; members } ->
      List.concat_map
        (fun (m : Ast.member) -> resolve_ty ~params ~tbl m.mtype)
        members
  | Ast.DUnion { params; variants } ->
      List.concat_map
        (fun (v : Ast.union_variant) ->
          match v.vpayload with
          | Some t -> resolve_ty ~params ~tbl t
          | None -> [])
        variants
  | Ast.DEnum _ -> [] (* enum cases are scalar; no type references *)
  | Ast.DOp { input; output } ->
      let opt = function
        | Some t -> resolve_ty ~params:[] ~tbl t
        | None -> []
      in
      opt input @ opt output

let resolve_decls (tbl : Symtab.t) (file : Ast.file) : Diagnostic.t list =
  List.concat_map (resolve_decl tbl) file
