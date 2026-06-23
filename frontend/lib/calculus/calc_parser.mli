(* Parse calculus source (a sequence of function definitions) into a program
   plus diagnostics, in source order. Never raises; syntax errors become
   diagnostics and leave [EError] placeholders. *)

val parse : string -> Calc_ast.program * Diagnostic.t list
