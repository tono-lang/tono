(* HTTP binding validation for operations carrying @http: unmatched path
   placeholders vs @httpLabel members (TC0019), an @httpPayload member sharing
   the body with an unmarked member or a second @httpPayload (TC0020), and a
   map-typed member bound to a query parameter or header (TC0021). Reports at the
   AST level so diagnostics carry source spans; [Protocol_http] materializes the
   descriptor assuming these have passed. *)
val check_decls : Ast.decl list -> Diagnostic.t list
