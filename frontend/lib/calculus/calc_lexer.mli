(* Tokenize calculus source into tokens plus lexical diagnostics, in source
   order. Never raises; lexical errors become diagnostics. *)

val tokenize : string -> Calc_token.t list * Diagnostic.t list
