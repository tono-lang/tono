(* The typechecker: a validation pass over the lowered IR. It takes the surface
   [Ast.file] alongside the [Ir.module_] because diagnostics need source spans
   (the IR carries none). It returns the module unchanged plus any diagnostics;
   it never raises and accumulates all findings (no fail-fast). *)

val check_module : file:Ast.file -> Ir.module_ -> Ir.module_ * Diagnostic.t list
