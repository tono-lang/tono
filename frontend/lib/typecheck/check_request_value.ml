(* The position rule for `.request`: where the canonical, already-assembled
   request may be read as an extern-call argument. Split out of
   [Check_entry_ops] to keep that file under the line-count cap. *)

let err code span fmt = Printf.ksprintf (Diagnostic.error ~code span) fmt

(* `.request` is the canonical, already-assembled request; it exists only
   once a @header/@body extern call reads it, never before. Legal: a direct
   or ctor-nested [CaRef ["request"]] argument to an [Ast.ACall] that is
   itself the value of a header/body trait. Everywhere else — bare (not
   passed to a call), inside another trait (@http, @query, @timeout,
   @retry, @errors, ...), or passed to a call in a non-header/body position
   — is an error: reading it there would mean a request that has not been
   built yet. @query is excluded on purpose, not merely unimplemented: the
   URL it feeds is already finalized before a call could read the assembled
   request, so `.request` has nothing to read there (the target emitters'
   own generation-time validation rejects a call in this position for the
   same reason). A bare [CaParam] inside the call's own arguments is also
   rejected: unlike an "ext" block's own [call:] line, a trait argument's
   call has no extern-side parameter list to forward a bare identifier
   from. *)
let request_trait_names = [ "header"; "body" ]

let check_request_value (op : Ast.decl) : Diagnostic.t list =
  let bad_bare span =
    err Error_codes.request_value_invalid span
      "'.request' is only valid as an argument to an extern call inside \
       @header/@body"
  in
  let bad_param span =
    err Error_codes.request_value_invalid span
      "a bare identifier has no meaning here; pass a literal or a field \
       reference"
  in
  (* [allow_request] is true only inside the argument subtree of an
     [Ast.ACall] reached directly from a header/body trait's value: a
     nested ctor/list/call within that subtree still counts (a signer that
     forwards `.request` into a nested struct-literal mapper is still
     "argument of a trait of request"), but stepping into a *different*
     top-level trait argument, or a call anywhere outside that subtree,
     resets it to false. *)
  let rec value_diags ~allow_request (v : Ast.trait_arg) : Diagnostic.t list =
    match v with
    | Ast.ARef r when r.Ast.segs = [ "request" ] ->
        if allow_request then [] else [ bad_bare r.Ast.ref_span ]
    | Ast.ARef _ -> []
    | Ast.AKv (_, v) -> value_diags ~allow_request v
    | Ast.AList xs -> List.concat_map (value_diags ~allow_request) xs
    | Ast.ACtor c ->
        List.concat_map
          (fun (_, _, v) -> value_diags ~allow_request v)
          c.Ast.ctor_fields
    | Ast.ACall ce ->
        List.concat_map (call_arg_diags ~allow_request) ce.Ast.ce_args
    | Ast.AString _ | Ast.AInt _ | Ast.AFloat _ | Ast.AName _ -> []
  and call_arg_diags ~allow_request (a : Ast.call_arg) : Diagnostic.t list =
    match a with
    | Ast.CaRef r when r.Ast.segs = [ "request" ] ->
        if allow_request then [] else [ bad_bare r.Ast.ref_span ]
    | Ast.CaRef _ | Ast.CaLit _ -> []
    | Ast.CaParam (_, span) -> [ bad_param span ]
    | Ast.CaCtor c ->
        List.concat_map
          (fun (_, _, v) -> value_diags ~allow_request v)
          c.Ast.ctor_fields
  in
  List.concat_map
    (fun (tr : Ast.trait) ->
      let is_request_trait = List.mem tr.Ast.tname request_trait_names in
      (* A "key: value"-form top-level argument is unwrapped before the
         request-trait dispatch, so @header(value: ns.fn(.request)) reaches
         the same allow-zone as the ordinary positional spelling. *)
      let rec top (v : Ast.trait_arg) : Diagnostic.t list =
        match v with
        | Ast.AKv (_, v) -> top v
        | Ast.ACall ce when is_request_trait ->
            List.concat_map (call_arg_diags ~allow_request:true) ce.Ast.ce_args
        | v -> value_diags ~allow_request:false v
      in
      List.concat_map top tr.Ast.targs)
    op.Ast.dtraits
