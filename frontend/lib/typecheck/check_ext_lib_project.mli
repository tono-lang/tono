(* Cross-file closed accounting for the "ext"/"extern" FFI library block
   (decision K). See check_ext_lib_project.ml for the rule-by-rule
   commentary. *)

(* Every module's own (name, decls) pair, keyed the same way project tooling
   names a module. Needs every module at once, so callers run it themselves
   alongside [Typecheck.check_module] (mirroring [Modules.build]) rather
   than through [Check_ext_lib.check_decls]: [Tono_frontend.compile] with a
   single [("", decls)] entry for a lone module, [Tono_frontend.compile_project]
   with the whole file set. A diagnostic implicating a named file is
   prefixed "name: "; the "" name gets no prefix. *)
val check_project : (string * Ast.decl list) list -> Diagnostic.t list
