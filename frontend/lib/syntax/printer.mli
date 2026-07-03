(* Pretty-printer: re-emits a parsed file as canonical source text. Parsing the
   output yields the same AST (spans aside), the invariant `tono fmt` and the
   golden tests rely on. Comments are not preserved: the lexer discards them. *)

(* A double-quoted single-line literal, inverting the lexer's escape decoding. *)
val string_literal : string -> string

(* A float rendering the lexer accepts: optional '-', digits '.' digits, no
   exponent. The shortest such form that round-trips to the same float. *)
val float_literal : float -> string
val print_ty : Ast.ty -> string
val print_file : Ast.file -> string
