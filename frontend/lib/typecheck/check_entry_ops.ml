(* Operation-position rules of the entry model: protocol traits require
   @http, an entry @http op names its endpoint as a reachable string-field
   ref, @timeout/@retry are typed refs, @header keys/values obey the surface
   (templated keys, literal-or-ref values), and template strings in protocol
   positions parse with their real span so an unterminated "{" is diagnosed.
   A loose operation has no field scope: refs, endpoint:, and @timeout/@retry
   belong to an entry. *)

let err code span fmt = Printf.ksprintf (Diagnostic.error ~code span) fmt

let find_trait name (traits : Ast.trait list) : Ast.trait option =
  List.find_opt (fun (t : Ast.trait) -> String.equal t.tname name) traits

let traits_named name (traits : Ast.trait list) : Ast.trait list =
  List.filter (fun (t : Ast.trait) -> String.equal t.tname name) traits

let kv_arg key (args : Ast.trait_arg list) : Ast.trait_arg option =
  List.find_map
    (function Ast.AKv (k, v) when String.equal k key -> Some v | _ -> None)
    args

let base_ty = Entry_scope.base_ty
let resolve_path = Entry_scope.resolve_path
let path_str = Entry_scope.path_str
let scalar_of_ty = Entry_scope.scalar_of_ty
let protocol_trait_names = Entry_scope.protocol_trait_names
let op_refs = Entry_scope.op_refs

(* Protocol checks shared by every op: @header/@timeout/@retry require @http
   (a purely local operation has no protocol surface). *)
let check_protocol_positions (op : Ast.decl) : Diagnostic.t list =
  if Option.is_some (find_trait "http" op.dtraits) then []
  else
    List.filter_map
      (fun (tr : Ast.trait) ->
        if List.mem tr.Ast.tname protocol_trait_names then
          Some
            (err Error_codes.protocol_trait_invalid tr.tspan
               "@%s is a protocol trait and requires @http on the operation"
               tr.tname)
        else None)
      op.Ast.dtraits

(* A loose (non-entry) operation has no field scope: any field reference in a
   protocol trait, any endpoint:, and @timeout/@retry (which only take field
   references by definition) belong to an entry. *)
let check_loose_op (op : Ast.decl) : Diagnostic.t list =
  let entry_only what span =
    err Error_codes.protocol_trait_invalid span
      "%s is only available on an operation declared in an entry body" what
  in
  let ref_diags =
    List.map
      (fun ((segs : string list), span) ->
        entry_only (Printf.sprintf "field reference '%s'" (path_str segs)) span)
      (op_refs op)
  in
  let endpoint_diags =
    match find_trait "http" op.dtraits with
    | Some tr when Option.is_some (kv_arg "endpoint" tr.targs) -> (
        match kv_arg "endpoint" tr.targs with
        | Some (Ast.ARef _) -> [] (* already reported as a field reference *)
        | _ -> [ entry_only "endpoint:" tr.tspan ])
    | _ -> []
  in
  (* A literal @timeout(5)/@retry(3) would otherwise pass here and be dropped
     in silence by the protocol resolver. *)
  let timeout_retry_diags =
    List.concat_map
      (fun name ->
        List.filter_map
          (fun (tr : Ast.trait) ->
            match tr.Ast.targs with
            | [ Ast.ARef _ ] -> None (* already reported as a field reference *)
            | _ -> Some (entry_only ("@" ^ name) tr.tspan))
          (traits_named name op.dtraits))
      [ "timeout"; "retry" ]
  in
  (* @header still requires @http; @timeout/@retry are covered above. *)
  let header_http_diags =
    if Option.is_some (find_trait "http" op.dtraits) then []
    else
      List.map
        (fun (tr : Ast.trait) ->
          err Error_codes.protocol_trait_invalid tr.tspan
            "@header is a protocol trait and requires @http on the operation")
        (traits_named "header" op.dtraits)
  in
  header_http_diags @ ref_diags @ endpoint_diags @ timeout_retry_diags

let check_entry_op ctx (fields : Ast.member list) (op : Ast.decl) :
    Diagnostic.t list =
  let resolve segs = resolve_path ctx fields segs in
  let ref_diags =
    List.filter_map
      (fun (segs, span) ->
        if Option.is_some (resolve segs) then None
        else
          Some
            (err Error_codes.field_ref_unknown span
               "unknown field '%s' referenced by an operation trait"
               (path_str segs)))
      (op_refs op)
  in
  let http_diags =
    match find_trait "http" op.dtraits with
    | None -> []
    | Some http -> (
        match kv_arg "endpoint" http.targs with
        | None ->
            [
              err Error_codes.entry_endpoint_missing http.tspan
                "an entry operation's @http must name its endpoint, e.g. \
                 @http(..., endpoint: .endpoint)";
            ]
        | Some (Ast.ARef r) -> (
            match resolve r.segs with
            | Some m when scalar_of_ty ctx m.mtype = Entry_scope.SString -> []
            | Some _ ->
                [
                  err Error_codes.entry_endpoint_missing r.ref_span
                    "endpoint '%s' must reference a string field"
                    (path_str r.segs);
                ]
            | None -> [] (* unknown ref already reported above *))
        | Some _ ->
            [
              err Error_codes.entry_endpoint_missing http.tspan
                "endpoint: takes a field reference (.field), not a literal";
            ])
  in
  let typed_ref name want scalar_desc =
    List.concat_map
      (fun (tr : Ast.trait) ->
        match tr.Ast.targs with
        | [ Ast.ARef r ] -> (
            match resolve r.segs with
            | None -> [] (* already reported *)
            | Some m ->
                if want ctx m.mtype then []
                else
                  [
                    err Error_codes.protocol_trait_invalid r.ref_span
                      "@%s must reference a %s field" name scalar_desc;
                  ])
        | _ ->
            [
              err Error_codes.protocol_trait_invalid tr.tspan
                "@%s takes a single field reference, e.g. @%s(.field)" name name;
            ])
      (traits_named name op.dtraits)
  in
  let timeout_diags =
    typed_ref "timeout"
      (fun _ t ->
        match base_ty t with Ast.TPrim ("duration", _) -> true | _ -> false)
      "duration"
  in
  let retry_diags =
    typed_ref "retry"
      (fun ctx t -> scalar_of_ty ctx t = Entry_scope.SInt)
      "integer"
  in
  (* Template strings in protocol positions parse here with their real span,
     so an unterminated "{" is diagnosed instead of silently going literal. *)
  let template_of ~span str =
    let d = ref [] in
    let parts = Lower.parse_template ~diags:d ~span str in
    (parts, List.rev !d)
  in
  let path_template_diags =
    match find_trait "http" op.dtraits with
    | Some { targs; tspan; _ } -> (
        match kv_arg "path" targs with
        | Some (Ast.AString p) -> snd (template_of ~span:tspan p)
        | _ -> [])
    | None -> []
  in
  let header_diags =
    List.concat_map
      (fun (tr : Ast.trait) ->
        match tr.Ast.targs with
        | [ key; value ] -> (
            (match key with
              | Ast.AString k ->
                  let parts, diags = template_of ~span:tr.tspan k in
                  diags
                  @
                  (* Input members vary per call; only the @http path carries
                   that scope. A header key resolves at construction. *)
                  if
                    List.exists
                      (function Ir.Tpl_input _ -> true | _ -> false)
                      parts
                  then
                    [
                      err Error_codes.protocol_trait_invalid tr.tspan
                        "input placeholders ({name}) are only available in the \
                         @http path; a @header key takes {.field} references";
                    ]
                  else []
              | _ ->
                  [
                    err Error_codes.protocol_trait_invalid tr.tspan
                      "@header expects a string key (a literal, possibly with \
                       {.field} placeholders)";
                  ])
            @
            (* The value forms are a literal string or a field reference; a
               template here is out of surface (derive a @format field and
               reference it), and a non-string literal has no defined
               stringification on the wire. *)
            match value with
            | Ast.AString v -> (
                let parts, diags = template_of ~span:tr.tspan v in
                diags
                @
                match parts with
                | [] | [ Ir.Tpl_lit _ ] -> []
                | _ ->
                    [
                      err Error_codes.protocol_trait_invalid tr.tspan
                        "a template in a @header value is not supported; \
                         derive it with @format on a field and reference the \
                         field";
                    ])
            | Ast.ARef _ -> []
            | _ ->
                [
                  err Error_codes.protocol_trait_invalid tr.tspan
                    "@header expects a string literal or a field reference as \
                     its value";
                ])
        | _ ->
            [
              err Error_codes.protocol_trait_invalid tr.tspan
                "@header expects a key and a value, e.g. @header(\"X-Name\", \
                 .field)";
            ])
      (traits_named "header" op.dtraits)
  in
  check_protocol_positions op
  @ ref_diags @ http_diags @ path_template_diags @ timeout_diags @ retry_diags
  @ header_diags
