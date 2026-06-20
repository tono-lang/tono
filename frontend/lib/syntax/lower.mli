(* Lowering of the surface AST to the IR. Diagnostics (e.g. [decimal] used as a
   type) are appended to the shared sink. *)

(* [lower_type ~params ~diags t] resolves a surface type to an IR [tref];
   [params] are the type-parameter names in scope. *)
val lower_type :
  params:string list -> diags:Diagnostic.t list ref -> Ast.ty -> Ir.tref

(* Canonical declared names must be snake_case. *)
val is_snake_case : string -> bool

(* Lower a surface member, lifting core constraints and routing other traits to
   the bag, with [params] the type parameters in scope. *)
val lower_member :
  params:string list -> diags:Diagnostic.t list ref -> Ast.member -> Ir.member

(* Lower a surface declaration to an IR shape. *)
val lower_decl : diags:Diagnostic.t list ref -> Ast.decl -> Ir.shape

(* Implementation details surfaced for unit testing. *)
module Internal : sig
  (* Map a primitive keyword (as the lexer recognizes it) to an IR primitive. *)
  val prim_of_keyword : string -> Ir.prim
end
