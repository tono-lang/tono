(* Template strings with placeholders, shared by lowering (@format), the
   typechecker (protocol positions), and the Protocol resolver: "{.a.b}" is an
   entry-field placeholder, "{name}" an operation-input member placeholder,
   everything else literal. Degenerate placeholders ("{}", "{.}", empty path
   segments) and an unterminated "{" are diagnosed and kept literal. *)

let report diags (d : Diagnostic.t) = diags := d :: !diags

let parse ~diags ~span (s : string) : Ir.template_part list =
  let n = String.length s in
  let parts = ref [] in
  let lit = Buffer.create 16 in
  let flush () =
    if Buffer.length lit > 0 then (
      parts := Ir.Tpl_lit (Buffer.contents lit) :: !parts;
      Buffer.clear lit)
  in
  let rec go i =
    if i >= n then flush ()
    else if s.[i] = '{' then (
      match String.index_from_opt s i '}' with
      | None ->
          report diags
            (Diagnostic.error span "unterminated '{' placeholder in template");
          Buffer.add_substring lit s i (n - i);
          flush ()
      | Some j ->
          flush ();
          let ph = String.sub s (i + 1) (j - i - 1) in
          if String.equal ph "" then
            report diags
              (Diagnostic.error span "empty '{}' placeholder in template")
          else if ph.[0] = '.' then
            let path =
              String.split_on_char '.' (String.sub ph 1 (String.length ph - 1))
            in
            if List.exists (fun seg -> String.equal seg "") path then
              report diags
                (Diagnostic.error span
                   "empty field reference in a '{...}' placeholder")
            else parts := Ir.Tpl_field path :: !parts
          else parts := Ir.Tpl_input ph :: !parts;
          go (j + 1))
    else (
      Buffer.add_char lit s.[i];
      go (i + 1))
  in
  go 0;
  List.rev !parts
