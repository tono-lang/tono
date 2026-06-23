(* Resolve every type reference in a file against the symbol table plus the
   in-scope generic parameters of the enclosing shape. An unresolved name is a
   TC0001; a generic applied with the wrong number of arguments is a TC0004; type
   arguments on a non-generic shape or a type parameter are a TC0005. *)

let err code span fmt = Printf.ksprintf (Diagnostic.error ~code span) fmt

(* Validate the head of a [name<args>] reference (the arguments are resolved
   separately). [name] is a type parameter, a declared shape, or unknown. *)
let resolve_head ~params ~(tbl : Symtab.t) name n_args span : Diagnostic.t list
    =
  if List.mem name params then
    (* A type parameter is opaque: bare use resolves, but it has no parameters of
       its own, so applying type arguments to it is non-generic application. *)
    if n_args = 0 then []
    else
      [
        err Error_codes.non_generic_applied span
          "type parameter '%s' is not generic and takes no type arguments" name;
      ]
  else
    match Symtab.find name tbl with
    | None -> [ err Error_codes.unknown_type span "unknown type '%s'" name ]
    | Some { arity; _ } ->
        if arity = 0 then
          if n_args = 0 then []
          else
            [
              err Error_codes.non_generic_applied span
                "'%s' is not generic and takes no type arguments" name;
            ]
        else if n_args = arity then []
        else
          [
            err Error_codes.generic_arity_mismatch span
              "'%s' expects %d type argument(s), but %d %s given" name arity
              n_args
              (if n_args = 1 then "was" else "were");
          ]

let rec resolve_ty ~params ~(tbl : Symtab.t) (ty : Ast.ty) : Diagnostic.t list =
  match ty with
  | Ast.TPrim _ -> [] (* the parser only emits a recognized primitive here *)
  | Ast.TError _ -> [] (* a parse error was already reported *)
  | Ast.TList (t, _) | Ast.TNullable (t, _) -> resolve_ty ~params ~tbl t
  | Ast.TMap (k, v, _) -> resolve_ty ~params ~tbl k @ resolve_ty ~params ~tbl v
  | Ast.TName (name, args, span) ->
      let arg_diags = List.concat_map (resolve_ty ~params ~tbl) args in
      resolve_head ~params ~tbl name (List.length args) span @ arg_diags

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
