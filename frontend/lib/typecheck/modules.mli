(* Project-level module machinery: per-module symbol tables, import maps, and the
   cross-module reference resolution over them. A project is a set of named
   modules (the name is the dotted file path); references between modules go
   through imports, and the import graph must be a DAG. *)

type index

(* The qualifier a reference uses for an import (its alias, else the last path
   segment) and the dotted target module name. *)
val qualifier_of : Ast.import -> string
val target_of : Ast.import -> string

(* Build the index from every module's parsed file. Collects per-module
   duplicate-shape diagnostics, flags imports whose target module does not exist
   (TC0019), and detects import cycles (TC0021). *)
val build : (string * Ast.file) list -> index * Diagnostic.t list

(* The reference resolver lowering uses to qualify a module's ids and references
   into "module#name" form, following [this_module]'s import map. *)
val resolver : index -> this_module:string -> Lower.ref_resolver

(* The cross-module reference check the typechecker plugs into [Resolve]: an
   unknown qualifier is TC0019, a non-[pub] target is TC0020, and generic arity
   is verified against the target module's declaration. *)
val qualified : index -> this_module:string -> Resolve.qualified
