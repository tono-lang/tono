(* Constraint and default validation over the lowered IR (with the surface AST
   for spans): core constraints must be type-compatible (TC0010) and well-formed
   (TC0011), and a member's default must match its type (TC0012) and satisfy its
   constraints (TC0013). *)

(* Check every struct member's constraints and default. *)
val check : file:Ast.file -> Ir.module_ -> Diagnostic.t list
