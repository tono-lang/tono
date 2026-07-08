(* Symbol table over one module's declarations. Cross-module resolution builds on
   top of these per-module tables (see [Modules]); within a module every shape
   lives in one flat namespace. *)

(* A declared shape: its generic arity (0 for non-generic shapes), whether it is
   [pub] (exported and visible to other modules), and the span of its name, used
   to point diagnostics at the original definition. *)
type entry = { arity : int; pub : bool; decl_span : Span.span }
type t

(* Build the table, reporting a duplicate-shape diagnostic (pointing at the
   redefinition, naming the first definition) for any repeated name. The first
   definition wins so later lookups resolve to it. *)
val build : Ast.decl list -> t * Diagnostic.t list

(* Look up a declared shape by name. *)
val find : string -> t -> entry option
