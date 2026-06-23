(* Global symbol table over a single file's declarations. Cross-module resolution
   is a later concern; here every shape lives in one flat namespace. *)

(* A declared shape: its generic arity (0 for non-generic shapes) and the span of
   its name, used to point diagnostics at the original definition. *)
type entry = { arity : int; decl_span : Span.span }
type t

(* Build the table, reporting a duplicate-shape diagnostic (pointing at the
   redefinition, naming the first definition) for any repeated name. The first
   definition wins so later lookups resolve to it. *)
val build : Ast.file -> t * Diagnostic.t list

(* Look up a declared shape by name. *)
val find : string -> t -> entry option
