(* Project-level module machinery: per-module symbol tables, import maps, and the
   cross-module reference resolution over them. A project is a set of named
   modules (the name is the dotted file path); references between modules go
   through imports, and the import graph must be a DAG. *)

type index

(* The qualifier a reference uses for an import (its alias, else the last path
   segment) and the dotted target module name. *)
val qualifier_of : Ast.import -> string
val target_of : Ast.import -> string

(* Build the index from every module's parsed file. Flags imports whose target
   module does not exist (TC0023), colliding import qualifiers (TC0026), and
   import cycles (TC0025). Duplicate-shape names are left to the typechecker's own
   per-module pass, so they are not double-reported. *)
val build : (string * Ast.file) list -> index * Diagnostic.t list

(* [build] with each diagnostic paired with the module whose imports raised it
   and the message unlabeled, for tooling that publishes per file. *)
val build_attributed :
  (string * Ast.file) list -> index * (string * Diagnostic.t) list

(* The reference resolver lowering uses to qualify a module's ids and references
   into "module#name" form, following [this_module]'s import map. *)
val resolver : index -> this_module:string -> Lower.ref_resolver

(* The cross-module reference check the typechecker plugs into [Resolve]: an
   unknown qualifier is TC0023, a non-[pub] target is TC0024, and generic arity
   is verified against the target module's declaration. *)
val qualified : index -> this_module:string -> Resolve.qualified
