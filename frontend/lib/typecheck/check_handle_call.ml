(* A call into a declared opaque handle's method, ".field.method(args)":
   the receiver must resolve to an entry field whose type is a declared
   opaque handle, the method must be one that handle declares, the argument
   count must match the method's declared logical parameters, and every
   argument must be a literal or a resolvable reference. Two surface
   positions share these rules: an op's own "impl" body (where a reference
   may also name the op's own parameter) and a field's own value source
   (where the field's type must be the method's declared return). *)

let err code span fmt = Printf.ksprintf (Diagnostic.error ~code span) fmt
let base_ty = Entry_scope.base_ty
let path_str = Entry_scope.path_str

(* The declared opaque type a foreign role's qualified name points at.
   [Roles.classify] only says a "qualifier.name" pair is Foreign; resolving
   it to the actual [Ast.opaque_type] (to read its declared methods) means
   scanning the "ext" block named [qualifier] for it. *)
let find_opaque_type (decls : Ast.decl list) ~(qualifier : string)
    ~(name : string) : Ast.opaque_type option =
  List.find_map
    (fun (d : Ast.decl) ->
      if not (String.equal d.Ast.dname qualifier) then None
      else
        match d.Ast.dkind with
        | Ast.DExtLib { body; _ } ->
            List.find_opt
              (fun (t : Ast.opaque_type) -> String.equal t.Ast.opq_name name)
              body.Ast.elib_types
        | _ -> None)
    decls

(* Every argument to the handle's method must resolve like any other
   operation ref (a bare identifier has no meaning here: unlike inside an
   "ext" block's own call:, there is no extern-side parameter list to
   forward from). *)
let rec check_arg ctx ~fields ~pname ~pty (a : Ast.call_arg) : Diagnostic.t list
    =
  match a with
  | Ast.CaRef r -> (
      match Entry_scope.resolve_ref ctx fields ~pname ~pty r.Ast.segs with
      | Some _ -> []
      | None ->
          [
            err Error_codes.field_ref_unknown r.Ast.ref_span
              "unknown field '%s'" (path_str r.Ast.segs);
          ])
  | Ast.CaLit _ -> []
  | Ast.CaParam (_, span) ->
      [
        err Error_codes.op_impl_arity_mismatch span
          "a bare identifier has no meaning here; pass a literal or a field \
           reference";
      ]
  | Ast.CaCtor c ->
      List.concat_map
        (fun (_, _, v) -> check_trait_arg ctx ~fields ~pname ~pty v)
        c.Ast.ctor_fields

and check_trait_arg ctx ~fields ~pname ~pty (v : Ast.trait_arg) :
    Diagnostic.t list =
  match v with
  | Ast.ARef r -> (
      match Entry_scope.resolve_ref ctx fields ~pname ~pty r.Ast.segs with
      | Some _ -> []
      | None ->
          [
            err Error_codes.field_ref_unknown r.Ast.ref_span
              "unknown field '%s'" (path_str r.Ast.segs);
          ])
  | Ast.AKv (_, v) -> check_trait_arg ctx ~fields ~pname ~pty v
  | Ast.AList xs -> List.concat_map (check_trait_arg ctx ~fields ~pname ~pty) xs
  | Ast.ACtor c ->
      List.concat_map
        (fun (_, _, v) -> check_trait_arg ctx ~fields ~pname ~pty v)
        c.Ast.ctor_fields
  | Ast.ACall _ ->
      [] (* a nested "ns.fn(...)" call resolves within its own ext block *)
  | Ast.AString _ | Ast.AInt _ | Ast.AFloat _ | Ast.AName _ -> []

(* Resolve the call against the handle its receiver names. Returns the
   method's declared logical return type when receiver and method both
   resolve (so a field position can compare it against the field's own
   type), alongside every diagnostic. [what] names the surrounding form
   ("'impl'" or "'='") in the receiver messages; [pname]/[pty] are the op's
   own parameter in an op body, [None] in a field position. *)
let check ctx ~(fields : Ast.member list) ~pname ~pty ~(what : string)
    (hc : Ast.op_impl) : Ast.ty option * Diagnostic.t list =
  let recv = hc.Ast.oi_recv in
  match Entry_scope.resolve_ref ctx fields ~pname ~pty recv.Ast.segs with
  | None ->
      ( None,
        [
          err Error_codes.field_ref_unknown recv.Ast.ref_span
            "unknown field '%s'" (path_str recv.Ast.segs);
        ] )
  | Some (Entry_scope.RParam _) ->
      ( None,
        [
          err Error_codes.op_impl_receiver_not_handle recv.Ast.ref_span
            "%s calls a method on an entry field; '%s' is the operation's own \
             parameter"
            what (path_str recv.Ast.segs);
        ] )
  | Some (Entry_scope.RField m) -> (
      match base_ty m.Ast.mtype with
      | Ast.TQName (qualifier, name, [], _)
        when Entry_scope.role_of_name ctx (qualifier ^ "." ^ name)
             = Roles.Foreign -> (
          match find_opaque_type ctx.Entry_scope.decls ~qualifier ~name with
          | None -> (None, [])
          (* classified Foreign but the opaque_type itself was not
             found: unreachable in practice (Roles.classify only marks
             a qualified name Foreign by finding this same opaque_type),
             kept defensive rather than asserting. *)
          | Some opq -> (
              match
                List.find_opt
                  (fun (e : Ast.extern_decl) ->
                    String.equal e.Ast.ed_name hc.Ast.oi_method)
                  opq.Ast.opq_methods
              with
              | None ->
                  ( None,
                    [
                      err Error_codes.op_impl_unknown_method
                        hc.Ast.oi_method_span
                        "'%s.%s' has no method '%s'; it declares: %s" qualifier
                        name hc.Ast.oi_method
                        (String.concat ", "
                           (List.map
                              (fun (e : Ast.extern_decl) -> e.Ast.ed_name)
                              opq.Ast.opq_methods));
                    ] )
              | Some meth ->
                  let arity_diags =
                    if
                      List.length hc.Ast.oi_args
                      <> List.length meth.Ast.ed_params
                    then
                      [
                        err Error_codes.op_impl_arity_mismatch hc.Ast.oi_span
                          "'%s.%s' takes %d argument(s), but %d %s given"
                          qualifier meth.Ast.ed_name
                          (List.length meth.Ast.ed_params)
                          (List.length hc.Ast.oi_args)
                          (if List.length hc.Ast.oi_args = 1 then "was"
                           else "were");
                      ]
                    else []
                  in
                  ( Some meth.Ast.ed_return,
                    arity_diags
                    @ List.concat_map
                        (check_arg ctx ~fields ~pname ~pty)
                        hc.Ast.oi_args )))
      | _ ->
          ( None,
            [
              err Error_codes.op_impl_receiver_not_handle recv.Ast.ref_span
                "'%s' is not a declared opaque handle; %s only calls a method \
                 on an entry field whose type is one"
                (path_str recv.Ast.segs) what;
            ] ))

(* A field's own "= .field.method(args)" value source: the shared call
   rules, plus the field's declared type must be the method's declared
   logical return (the value is stored as-is; a projection would need a
   name to live under, and that is what a "returns:" is for). *)
let check_field_source ctx ~(fields : Ast.member list) (m : Ast.member)
    (hc : Ast.op_impl) : Diagnostic.t list =
  let ret, diags = check ctx ~fields ~pname:None ~pty:None ~what:"'='" hc in
  let type_diags =
    match ret with
    | Some ret
      when not
             (String.equal (Printer.print_ty ret)
                (Printer.print_ty m.Ast.mtype)) ->
        [
          err Error_codes.handle_call_type_mismatch hc.Ast.oi_span
            "'%s' is declared as '%s', but '.%s.%s' returns '%s'" m.Ast.mname
            (Printer.print_ty m.Ast.mtype)
            (path_str hc.Ast.oi_recv.Ast.segs)
            hc.Ast.oi_method (Printer.print_ty ret);
        ]
    | _ -> []
  in
  diags @ type_diags
