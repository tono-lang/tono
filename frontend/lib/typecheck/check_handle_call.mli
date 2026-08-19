(* The receiver/method/arity/argument rules of a call into a declared opaque
   handle's method (".field.method(args)"), shared by an op's own "impl" body
   and a field's own value source (TC0038/TC0082/TC0083/TC0084/TC0094). *)

(* The declared opaque type a foreign role's qualified name points at. *)
val find_opaque_type :
  Ast.decl list -> qualifier:string -> name:string -> Ast.opaque_type option

(* Resolve one call: the method's declared return type when receiver and
   method resolve, plus every diagnostic. [what] names the surrounding form
   in messages; [pname]/[pty] are the op's own parameter (an op body) or
   [None] (a field position). *)
val check :
  Entry_scope.ctx ->
  fields:Ast.member list ->
  pname:string option ->
  pty:Ast.ty option ->
  what:string ->
  Ast.op_impl ->
  Ast.ty option * Diagnostic.t list

(* A field's own "= .field.method(args)" source: [check] plus the field's
   type must be the method's declared return. *)
val check_field_source :
  Entry_scope.ctx ->
  fields:Ast.member list ->
  Ast.member ->
  Ast.op_impl ->
  Diagnostic.t list
