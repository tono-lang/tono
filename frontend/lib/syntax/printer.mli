(* Pretty-printer: re-emits a parsed file as canonical source text. Parsing the
   output yields the same AST (spans aside), the invariant `tono fmt` and the
   golden tests rely on. Comments are not preserved: the lexer discards them. *)

(* A double-quoted single-line literal, inverting the lexer's escape decoding. *)
val string_literal : string -> string

(* A float rendering the lexer accepts: optional '-', digits '.' digits, no
   exponent. The shortest such form that round-trips to the same float. *)
val float_literal : float -> string
val print_ty : Ast.ty -> string
val print_trait : Ast.trait -> string

(* One member line, indented as it appears in a shape body: type, an optional
   selection table, then its traits. Exposed so tooling (hover) renders a
   member exactly as `tono fmt` writes it. *)
val print_member : Ast.member -> string
val print_file : Ast.file -> string
