(* Placeholder-template parsing shared by lowering, the typechecker, and the
   Protocol resolver: "{.a.b}" is an entry-field placeholder, "{name}" an
   operation-input member placeholder, everything else literal. Malformed
   placeholders are diagnosed and kept literal. *)

val parse :
  diags:Diagnostic.t list ref ->
  span:Span.span ->
  string ->
  Ir.template_part list
