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

(* A field reference as written, including a trailing map index
   ("base[key]"). Exposed so tooling (hover) renders a match-indexed subject
   exactly as `tono fmt` writes it. *)
val print_ref : Ast.ref_path -> string

(* One member line, indented as it appears in a shape body: type, an optional
   selection table, then its traits. Exposed so tooling (hover) renders a
   member exactly as `tono fmt` writes it. *)
val print_member : Ast.member -> string
val print_file : Ast.file -> string

(* One extern declaration (a free function or an opaque handle's method) and
   one opaque handle, each with its language blocks, as written inside an
   ext body at [indent]. Exposed so tooling renders them as `tono fmt` does. *)
val print_extern : indent:string -> Ast.extern_decl -> string
val print_opaque_type : indent:string -> Ast.opaque_type -> string
