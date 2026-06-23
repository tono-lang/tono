(* Type-check a calculus program against the IR module that supplies the nominal
   types it references. Returns diagnostics (with stable CAxxxx codes) in source
   order; never raises and accumulates every finding. *)

val check : Ir.module_ -> Calc_ast.program -> Diagnostic.t list
