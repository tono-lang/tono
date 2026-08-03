(* The typechecker: a validation pass over the lowered IR. It takes the surface
   [Ast.file] alongside the [Ir.module_] because diagnostics need source spans
   (the IR carries none). It returns the module unchanged plus any diagnostics;
   it never raises and accumulates all findings (no fail-fast). [qualified]
   resolves cross-module references; the project pipeline supplies one backed by
   the import map, otherwise a lone module has no imports (the default). *)

let check_module ?(qualified = Resolve.no_imports) ~(file : Ast.file)
    (m : Ir.module_) : Ir.module_ * Diagnostic.t list =
  let decls = file.Ast.decls in
  let tbl, dup_diags = Symtab.build decls in
  let ref_diags = Resolve.resolve_decls ~qualified tbl decls in
  let member_diags = Check_member.check_decls decls in
  let enum_diags = Check_enum.check_decls decls in
  let constraint_diags = Check_constraints.check ~decls m in
  let op_diags = Check_operations.check_decls tbl decls in
  let http_diags = Check_http.check_decls decls in
  let ext_diags = Check_ext.check_decls decls in
  let impl_diags = Check_impl.check_decls decls in
  let entry_diags = Check_entries.check_decls decls in
  let repeat_diags = Check_trait_repeats.check_decls decls in
  let trait_name_diags = Check_trait_names.check_decls decls in
  ( m,
    Diagnostic.sort
      (dup_diags @ ref_diags @ member_diags @ enum_diags @ constraint_diags
     @ op_diags @ http_diags @ ext_diags @ impl_diags @ entry_diags
     @ repeat_diags @ trait_name_diags) )
