(* Resolve every type reference in a module against the symbol table plus the
   in-scope generic parameters of the enclosing shape. An unresolved name is a
   TC0001; a generic applied with the wrong number of arguments is a TC0004; type
   arguments on a non-generic shape or a type parameter are a TC0005. Qualified
   references ([qualifier.Name]) are resolved by the [qualified] callback, which
   the project pipeline supplies with the import map; on its own a module has no
   imports, so a qualified reference is an unknown-import error. *)

let err code span fmt = Printf.ksprintf (Diagnostic.error ~code span) fmt

(* Resolution of a cross-module reference [qualifier.Name] applied to [n_args]
   type arguments. Returns the diagnostics for that reference (empty if it
   resolves). *)
type qualified =
  qualifier:string ->
  name:string ->
  n_args:int ->
  Span.span ->
  Diagnostic.t list

(* With no import map in scope, any qualified reference is unresolved: a module
   compiled on its own cannot name another. *)
let no_imports : qualified =
 fun ~qualifier ~name:_ ~n_args:_ span ->
  [
    err Error_codes.unknown_import span
      "unknown module qualifier '%s'; import the module to reference it"
      qualifier;
  ]

(* Validate the head of a [name<args>] reference (the arguments are resolved
   separately). [name] is a type parameter, a declared shape, a [known]
   non-shape name (an "ext" library's foreign form), or unknown. *)
let resolve_head ~params ~(tbl : Symtab.t) ~(known : string -> bool) name n_args
    span : Diagnostic.t list =
  if String.equal name "decimal" then
    (* [decimal] is rejected during lowering with bespoke guidance (model money
       as minor units, or use float); do not also flag it as a generic unknown. *)
    []
  else if List.mem name params then
    (* A type parameter is opaque: bare use resolves, but it has no parameters of
       its own, so applying type arguments to it is non-generic application. *)
    if n_args = 0 then []
    else
      [
        err Error_codes.non_generic_applied span
          "type parameter '%s' is not generic and takes no type arguments" name;
      ]
  else if known name then
    (* Recognized outside the shape table (a foreign struct/opaque type): not
       unknown, and never generic. Whether this reference is even legal here
       (an "ext" foreign form is never wire, entry/op boundary, ...) is a
       different, closed-boundary question a later pass owns. *)
    if n_args = 0 then []
    else
      [
        err Error_codes.non_generic_applied span
          "'%s' is not generic and takes no type arguments" name;
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

let rec resolve_ty ~params ~(tbl : Symtab.t) ~(qualified : qualified)
    ?(known = fun _ -> false) (ty : Ast.ty) : Diagnostic.t list =
  match ty with
  | Ast.TPrim _ -> [] (* the parser only emits a recognized primitive here *)
  | Ast.TError _ -> [] (* a parse error was already reported *)
  | Ast.TList (t, _) | Ast.TNullable (t, _) ->
      resolve_ty ~params ~tbl ~qualified ~known t
  | Ast.TMap (k, v, _) ->
      resolve_ty ~params ~tbl ~qualified ~known k
      @ resolve_ty ~params ~tbl ~qualified ~known v
  | Ast.TName (name, args, span) ->
      let arg_diags =
        List.concat_map (resolve_ty ~params ~tbl ~qualified ~known) args
      in
      resolve_head ~params ~tbl ~known name (List.length args) span @ arg_diags
  | Ast.TQName (qualifier, name, args, span) ->
      let arg_diags =
        List.concat_map (resolve_ty ~params ~tbl ~qualified ~known) args
      in
      qualified ~qualifier ~name ~n_args:(List.length args) span @ arg_diags

let rec resolve_decl ~(qualified : qualified) ~(known : string -> bool)
    (tbl : Symtab.t) (d : Ast.decl) : Diagnostic.t list =
  match d.dkind with
  | Ast.DStruct { params; members; ops } ->
      List.concat_map
        (fun (m : Ast.member) ->
          resolve_ty ~params ~tbl ~qualified ~known m.mtype)
        members
      (* Ops nested in an entry body resolve exactly like top-level ops. *)
      @ List.concat_map (resolve_decl ~qualified ~known tbl) ops
  | Ast.DUnion { params; variants } ->
      List.concat_map
        (fun (v : Ast.union_variant) ->
          match v.vpayload with
          | Some t -> resolve_ty ~params ~tbl ~qualified ~known t
          | None -> [])
        variants
  | Ast.DEnum _ -> [] (* enum cases are scalar; no type references *)
  | Ast.DOp { pname = _; input; output; oimpl = _ } ->
      let opt = function
        | Some t -> resolve_ty ~params:[] ~tbl ~qualified ~known t
        | None -> []
      in
      opt input @ opt output
  (* A signature ref can name a runtime type (e.g. canonical_request) that is not
     a user shape and has no per-target registry here, so extension signatures are
     exempt from name resolution entirely. The tradeoff is that a plain typo of a
     user shape in a signature is not flagged either; binding-vs-signature
     validation (which would catch it) is deferred. *)
  | Ast.DExt _ -> []
  (* An ext lib's foreign structs/externs reference each other and the
     surrounding module, but that resolution is deferred (out of scope). *)
  | Ast.DExtLib _ -> []
  (* Test blocks resolve their own names (bindings, entries, ops) in
     [Check_tests], which owns the richer diagnostics. *)
  | Ast.DTest _ -> []

let resolve_decls ?(qualified = no_imports) ?(known = fun _ -> false)
    (tbl : Symtab.t) (decls : Ast.decl list) : Diagnostic.t list =
  List.concat_map (resolve_decl ~qualified ~known tbl) decls
