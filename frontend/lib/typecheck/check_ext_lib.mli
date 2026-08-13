(* Internal-consistency typecheck for the "ext"/"extern" FFI library block
   (RFC-0023): call arity/types against the declared logical signature,
   yields:/returns:/errors: closure, and the cross-file closed accounting of
   one "ext" split across several .tono files. See check_ext_lib.ml for the
   rule-by-rule commentary. Never verifies that a declared foreign symbol
   really exists in the target library: that is the target compiler's job
   (RFC's degrau 2), out of scope here. *)

(* Per-module pass. [tbl] resolves an "errors:" sentinel's declared type. *)
val check_decls : tbl:Symtab.t -> Ast.decl list -> Diagnostic.t list

(* An "ext <name> { ... }" block is also a namespace: "name.Type" qualifies
   a declared opaque type the same way an import qualifies a cross-module
   shape (companybus.publisher). [Resolve]'s generic [qualified] callback has
   no visibility into same-module ext lib names, so this wraps one: a
   qualifier/name pair that names a declared opaque type resolves with no
   diagnostic (opaque types take no type arguments); anything else falls
   through to [fallback] unchanged. Callers thread the result into
   [Resolve.resolve_decls] wherever they already thread [qualified]. *)
val qualified_of : Ast.decl list -> Resolve.qualified -> Resolve.qualified

(* The cross-file closed accounting (decision K): every module's own
   (name, decls) pair, keyed the same way project tooling names a module.
   Needs every module at once, so callers run it themselves alongside
   [Typecheck.check_module] (mirroring [Modules.build]) rather than through
   [check_decls]: [Tono_frontend.compile] with a single [("", decls)] entry
   for a lone module, [Tono_frontend.compile_project] with the whole file
   set. A diagnostic implicating a named file is prefixed "name: "; the ""
   name gets no prefix. *)
val check_project : (string * Ast.decl list) list -> Diagnostic.t list
