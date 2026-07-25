(* Lowering of the surface AST to the IR. Diagnostics (e.g. [decimal] used as a
   type) are appended to the shared sink. *)

(* Map a surface type reference to its fully-qualified IR shape id. [qualifier] is
   the import qualifier for a cross-module reference, or [None] for an in-module
   name. Lowering names the id it is told to; existence and visibility are the
   resolve pass's concern. *)
type ref_resolver = qualifier:string option -> name:string -> Ir.shape_id

(* [module#name], the canonical namespaced id; an empty module name leaves the
   name bare. *)
val qualify : string -> string -> Ir.shape_id

(* The default reference resolver for a file compiled on its own: an in-module
   name is qualified with [module_name], and a qualifier is taken as a module
   directly (the resolve pass then flags it as an unknown import). *)
val default_resolver : module_name:string -> ref_resolver

(* [lower_type ~params ~resolve ~diags t] resolves a surface type to an IR [tref];
   [params] are the type-parameter names in scope and [resolve] namespaces
   references. *)
val lower_type :
  params:string list ->
  resolve:ref_resolver ->
  diags:Diagnostic.t list ref ->
  Ast.ty ->
  Ir.tref

(* Canonical declared names must be snake_case. *)
val is_snake_case : string -> bool

(* Lower a surface member, lifting core constraints and routing other traits to
   the bag, with [params] the type parameters in scope. *)
val lower_member :
  params:string list ->
  resolve:ref_resolver ->
  diags:Diagnostic.t list ref ->
  Ast.member ->
  Ir.member

(* Parse a template string into IR parts: "{.a.b}" is an entry-field
   placeholder, "{name}" an operation-input member placeholder, everything else
   literal. An unterminated "{" is diagnosed and kept literal. *)
val parse_template :
  diags:Diagnostic.t list ref ->
  span:Span.span ->
  string ->
  Ir.template_part list

(* The transform named by a catalog trait ("str::trim" -> "trim"), or [None]
   for a non-catalog trait name. Catalog membership is a typecheck concern. *)
val transform_of : string -> string option

(* Lower a surface declaration to an IR shape; its own id is qualified with
   [module_name] and its references through [resolve]. [role] (default [Wire])
   selects the struct lowering: an [Entry] struct lowers its fields with their
   sources and nests its ops as [module#entry.op]; a [Config] lowers fields
   only. Raises on a [DExt] declaration, which has no shape; [lower_file]
   routes those to [lower_ext]. *)
val lower_decl :
  ?role:Roles.role ->
  module_name:string ->
  resolve:ref_resolver ->
  diags:Diagnostic.t list ref ->
  Ast.decl ->
  Ir.shape

(* Lower a surface [DExt] declaration to an IR extension. [resolve] namespaces
   the signature type refs (their existence is not checked: binding-vs-signature
   validation is deferred). *)
val lower_ext :
  resolve:ref_resolver ->
  diags:Diagnostic.t list ref ->
  Ast.decl ->
  Ir.extension

(* Lower a file of declarations into a module; [module_name] becomes its name and
   operations are separated from the other shapes. [resolve] defaults to
   [default_resolver ~module_name]. Imports carry no IR of their own. *)
val lower_file :
  module_name:string ->
  ?resolve:ref_resolver ->
  diags:Diagnostic.t list ref ->
  Ast.file ->
  Ir.module_

(* Implementation details surfaced for unit testing. *)
module Internal : sig
  (* Map a primitive keyword (as the lexer recognizes it) to an IR primitive. *)
  val prim_of_keyword : string -> Ir.prim
end
