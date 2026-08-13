(* Resolve every type reference in a module against the symbol table plus the
   in-scope generic parameters of the enclosing shape. An unresolved name is a
   TC0001; a generic applied with the wrong number of arguments is a TC0004; type
   arguments on a non-generic shape or a type parameter are a TC0005. *)

(* Resolution of a cross-module reference [qualifier.Name] applied to [n_args]
   type arguments; returns the diagnostics for that reference (empty if it
   resolves). The project pipeline supplies one backed by the import map; the
   default treats every qualified reference as an unknown import (TC0023). *)
type qualified =
  qualifier:string ->
  name:string ->
  n_args:int ->
  Span.span ->
  Diagnostic.t list

(* The default [qualified]: no imports are in scope, so any qualified reference is
   an unknown import. *)
val no_imports : qualified

(* Resolve a single surface type; [params] are the type-parameter names in scope
   of the enclosing shape. [known] recognizes a bare name outside [tbl] (an
   "ext" library's foreign struct/opaque type, in particular: never a shape,
   so never in [tbl], but not unknown either); it defaults to recognizing
   nothing, so an unresolved bare name is always TC0001 unless a caller opts
   in. A name [known] resolves takes no type arguments (TC0005 otherwise). *)
val resolve_ty :
  params:string list ->
  tbl:Symtab.t ->
  qualified:qualified ->
  ?known:(string -> bool) ->
  Ast.ty ->
  Diagnostic.t list

(* Resolve every type reference across one module's declarations. [qualified]
   defaults to [no_imports]; [known] defaults to recognizing nothing. *)
val resolve_decls :
  ?qualified:qualified ->
  ?known:(string -> bool) ->
  Symtab.t ->
  Ast.decl list ->
  Diagnostic.t list
