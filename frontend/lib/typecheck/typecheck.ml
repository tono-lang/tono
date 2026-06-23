(* The typechecker: a validation pass over the lowered IR. It takes the surface
   [Ast.file] alongside the [Ir.module_] because diagnostics need source spans
   (the IR carries none). It returns the module unchanged plus any diagnostics;
   it never raises and accumulates all findings (no fail-fast). *)

let check_module ~(file : Ast.file) (m : Ir.module_) :
    Ir.module_ * Diagnostic.t list =
  let tbl, dup_diags = Symtab.build file in
  let ref_diags = Resolve.resolve_decls tbl file in
  let member_diags = Check_member.check_decls file in
  let enum_diags = Check_enum.check_decls file in
  let constraint_diags = Check_constraints.check ~file m in
  (m, dup_diags @ ref_diags @ member_diags @ enum_diags @ constraint_diags)
