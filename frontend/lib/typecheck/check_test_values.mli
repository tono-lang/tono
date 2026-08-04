(* Typed lowering of test-block values to wire-form JSON, shared by the test
   checker and the pattern lowering. *)

type virtual_mod = Vhttp | Verrors

type ctx = {
  decls : Ast.decl list;
  virtuals : (string * virtual_mod) list;
  diags : Diagnostic.t list ref;
}

val dummy_span : Span.span
val report : ctx -> Diagnostic.t -> unit

(* Which language module an import brings in, when it is one ([tono.http],
   [tono.errors]). *)
val virtual_of_import : Ast.import -> virtual_mod option

val make_ctx :
  imports:Ast.import list ->
  decls:Ast.decl list ->
  diags:Diagnostic.t list ref ->
  ctx

val decl_by_name : ctx -> string -> Ast.decl option
val struct_members : ctx -> string -> Ast.member list option
val base_ty : Ast.ty -> Ast.ty
val find_trait : string -> Ast.trait list -> Ast.trait option
val member_optional : Ast.member -> bool

type head_kind = Huser of string | Hvirtual of virtual_mod * string | Hbad

(* Resolve a ctor head; a 2-segment head goes through the imported language
   modules, and a bare http.*/errors.* head without the import gets the
   import suggestion (TC0064). [code] tags the generic failure. *)
val resolve_head : ctx -> code:string -> Ast.value_head -> head_kind
val value_span : Ast.test_value -> Span.span

type ref_typer = base:string -> path:string list -> Span.span -> Ast.ty option

val ref_leaf : string -> string list -> Ir.json

(* Encode a test value against its declared type, reporting TC0059 on any
   mismatch and yielding a best-effort JSON so checking continues. *)
val encode_value : ctx -> refty:ref_typer -> Ast.ty -> Ast.test_value -> Ir.json
