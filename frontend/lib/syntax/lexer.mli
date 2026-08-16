(* Tokenize source text into a token stream (ending in [Eof]) plus any lexical
   diagnostics. Never raises. *)
val tokenize : string -> Token.t list * Diagnostic.t list

(* The primitive type names the lexer recognizes as [Prim] tokens. *)
val prims : string list
