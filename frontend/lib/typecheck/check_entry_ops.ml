(* Operation-position rules of the entry model: protocol traits require
   @http, an entry @http op names its endpoint as a reachable string-field
   ref, @timeout/@retry are typed refs, @header/@query keys and values share
   one grammar (literal, template, or pure reference), and template strings
   in protocol positions parse with their real span so an unterminated "{"
   is diagnosed. A loose operation has no entry-field scope, but it can still
   reference its own declared parameter: only entry-field refs (and
   endpoint:/@timeout/@retry, which only ever name entry fields) require an
   entry. *)

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
let path_str = Entry_scope.path_str
let scalar_of_ty = Entry_scope.scalar_of_ty
let op_refs = Entry_scope.op_refs
let op_param = Entry_scope.op_param

(* @query/@body join @header/@timeout/@retry as protocol traits requiring
   @http; their refs ride the same [op_refs] collection. *)
let protocol_trait_names = Entry_scope.protocol_trait_names

(* Template strings in protocol positions parse here with their real span,
   so an unterminated "{" is diagnosed instead of silently going literal. *)
let template_of ~span str =
  let d = ref [] in
  let parts = Template.parse ~diags:d ~span str in
  (parts, List.rev !d)

let has_crlf s = String.exists (function '\r' | '\n' -> true | _ -> false) s

(* CR/LF in a header/query key or value is the one position a raw value does
   real damage in (request smuggling via an injected line); every other
   literal run in a protocol position is free-form. *)
let crlf_diags ~what span (parts : Ir.template_part list) : Diagnostic.t list =
  List.filter_map
    (function
      | Ir.Tpl_lit s when has_crlf s ->
          Some
            (err Error_codes.protocol_trait_invalid span
               "%s must not contain a carriage return or line feed" what)
      | _ -> None)
    parts

(* The shared grammar of a @header/@query key or value: a string (literal or
   template, real placeholders resolved elsewhere by [op_refs]) or a pure
   reference. The legacy [{name}] input placeholder is excluded: that
   convention only exists in the @http path. *)
let check_kv_shape ~trait_name ~key_what ~value_what (tr : Ast.trait) :
    Diagnostic.t list =
  match tr.Ast.targs with
  | [ key; value ] -> (
      (match key with
        | Ast.AString k ->
            let parts, diags = template_of ~span:tr.tspan k in
            diags
            @ crlf_diags ~what:key_what tr.tspan parts
            @
            if
              List.exists (function Ir.Tpl_input _ -> true | _ -> false) parts
            then
              [
                err Error_codes.protocol_trait_invalid tr.tspan
                  "input placeholders ({name}) are only available in the @http \
                   path; %s"
                  key_what;
              ]
            else []
        | Ast.ARef _ -> []
        | _ ->
            [
              err Error_codes.protocol_trait_invalid tr.tspan
                "@%s expects a string key (a literal or {.field} template) or \
                 a field reference"
                trait_name;
            ])
      @
      match value with
      | Ast.AString v ->
          let parts, diags = template_of ~span:tr.tspan v in
          diags @ crlf_diags ~what:value_what tr.tspan parts
      | Ast.ARef _ -> []
      | _ ->
          [
            err Error_codes.protocol_trait_invalid tr.tspan
              "@%s expects a string literal/template or a field reference as \
               its value"
              trait_name;
          ])
  | _ ->
      [
        err Error_codes.protocol_trait_invalid tr.tspan
          "@%s expects a key and a value, e.g. @%s(\"X-Name\", .field)"
          trait_name trait_name;
      ]

(* Shape rules of @header, independent of any field scope, so loose ops obey
   them too. *)
let check_header_shapes (op : Ast.decl) : Diagnostic.t list =
  List.concat_map
    (check_kv_shape ~trait_name:"header"
       ~key_what:"a @header key takes {.field} references"
       ~value_what:"a @header value")
    (traits_named "header" op.Ast.dtraits)

(* Shape rules of @query, mirroring @header. *)
let check_query_shapes (op : Ast.decl) : Diagnostic.t list =
  List.concat_map
    (check_kv_shape ~trait_name:"query"
       ~key_what:"a @query key takes {.field} references"
       ~value_what:"a @query value")
    (traits_named "query" op.Ast.dtraits)

(* @http(code:) is well-formed only as an int or a non-empty list of ints.
   Anything else (an empty list, a non-int element, a non-int scalar) would
   otherwise fall silently into the resolver's "no code declared" default
   instead of the exact match the author wrote. *)
let check_code (op : Ast.decl) : Diagnostic.t list =
  let is_int = function Ast.AInt _ -> true | _ -> false in
  match find_trait "http" op.dtraits with
  | None -> []
  | Some { targs; tspan; _ } -> (
      match kv_arg "code" targs with
      | None -> []
      | Some (Ast.AInt _) -> []
      | Some (Ast.AList xs) when xs <> [] && List.for_all is_int xs -> []
      | Some _ ->
          [
            err Error_codes.http_code_invalid tspan
              "@http(code:) must be an int or a non-empty list of ints, e.g. \
               'code: 201' or 'code: [200, 207]'";
          ])

(* Shape rules of @body: at most one per operation, and its single argument
   is either a reference (the whole parameter, or one of its members) or a
   ctor mapper (a known struct name applied to field: value pairs, each
   value itself a reference/literal — the same grammar @header/@query values
   already accept; nesting a second ctor inside a field is not allowed, the
   same "zero new expressive power" the RFC calls for). *)
let check_body_field_value ~ctor_name (fname, fspan, (v : Ast.trait_arg)) :
    Diagnostic.t list =
  match v with
  | Ast.ARef _ -> []
  | Ast.AString s ->
      let parts, diags = template_of ~span:fspan s in
      diags
      @
      if List.exists (function Ir.Tpl_input _ -> true | _ -> false) parts then
        [
          err Error_codes.protocol_trait_invalid fspan
            "input placeholders ({name}) are only available in the @http path";
        ]
      else []
  | Ast.ACtor _ ->
      [
        err Error_codes.protocol_trait_invalid fspan
          "a ctor field cannot nest another ctor; '%s.%s' takes a reference or \
           a literal"
          ctor_name fname;
      ]
  | _ ->
      [
        err Error_codes.protocol_trait_invalid fspan
          "a ctor field takes a reference or a literal/template value";
      ]

let check_body_ctor ctx (c : Ast.ctor_arg) : Diagnostic.t list =
  match Entry_scope.struct_members ctx c.Ast.ctor_name with
  | None ->
      [
        err Error_codes.protocol_trait_invalid c.Ast.ctor_name_span
          "'%s' is not a known struct; @body's ctor mapper names a declared \
           struct"
          c.Ast.ctor_name;
      ]
  | Some members ->
      let seen = Hashtbl.create 8 in
      List.concat_map
        (fun ((fname, fspan, _) as field) ->
          let name_diags =
            if
              List.exists
                (fun (m : Ast.member) -> String.equal m.Ast.mname fname)
                members
            then []
            else
              [
                err Error_codes.protocol_trait_invalid fspan
                  "'%s' has no field '%s'" c.Ast.ctor_name fname;
              ]
          in
          let dup_diags =
            if Hashtbl.mem seen fname then
              [
                err Error_codes.protocol_trait_invalid fspan
                  "'%s' is mapped more than once in this ctor" fname;
              ]
            else (
              Hashtbl.add seen fname ();
              [])
          in
          name_diags @ dup_diags
          @ check_body_field_value ~ctor_name:c.Ast.ctor_name field)
        c.Ast.ctor_fields

let check_body_shapes ctx (op : Ast.decl) : Diagnostic.t list =
  let bodies = traits_named "body" op.Ast.dtraits in
  let arity_diags =
    match bodies with
    | _ :: (_ :: _ as extra) ->
        List.map
          (fun (tr : Ast.trait) ->
            err Error_codes.protocol_trait_invalid tr.Ast.tspan
              "at most one @body is allowed per operation")
          extra
    | _ -> []
  in
  let shape_diags =
    List.concat_map
      (fun (tr : Ast.trait) ->
        match tr.Ast.targs with
        | [ Ast.ARef _ ] -> []
        | [ Ast.ACtor c ] -> check_body_ctor ctx c
        | [ _ ] ->
            [
              err Error_codes.protocol_trait_invalid tr.Ast.tspan
                "@body expects a field reference (e.g. .input or \
                 .input.member) or a struct-literal mapper (e.g. note_body { \
                 title: .input.title })";
            ]
        | _ ->
            [
              err Error_codes.protocol_trait_invalid tr.Ast.tspan
                "@body expects exactly one argument";
            ])
      bodies
  in
  arity_diags @ shape_diags

(* Protocol checks shared by every op: @header/@query/@timeout/@retry require
   @http (a purely local operation has no protocol surface). *)
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

(* A loose (non-entry) operation has no entry-field scope, but it can still
   reference its own declared parameter (zero or one segment deep, matching
   what codegen renders): a ref that resolves as a param reference is legal;
   anything else — an entry-field ref, endpoint:, @timeout/@retry (which only
   ever name entry fields) — belongs to an entry. *)
let check_loose_op ctx (op : Ast.decl) : Diagnostic.t list =
  let pname, pty = op_param op in
  let is_param_ref segs =
    Option.is_some (Entry_scope.resolve_param ctx pname pty segs)
  in
  let entry_only what span =
    err Error_codes.protocol_trait_invalid span
      "%s is only available on an operation declared in an entry body" what
  in
  let ref_diags =
    List.filter_map
      (fun ((segs : string list), span) ->
        if is_param_ref segs then None
        else
          Some
            (entry_only
               (Printf.sprintf "field reference '%s'" (path_str segs))
               span))
      (op_refs op)
  in
  let endpoint_diags =
    match find_trait "http" op.dtraits with
    | Some tr when Option.is_some (kv_arg "endpoint" tr.targs) -> (
        match kv_arg "endpoint" tr.targs with
        | Some (Ast.ARef r) when is_param_ref r.segs -> []
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
            | [ Ast.ARef r ] when is_param_ref r.segs -> None
            | [ Ast.ARef _ ] -> None (* already reported as a field reference *)
            | _ -> Some (entry_only ("@" ^ name) tr.tspan))
          (traits_named name op.dtraits))
      [ "timeout"; "retry" ]
  in
  (* @header/@query/@body still require @http; @timeout/@retry are covered
     above. *)
  let protocol_http_diags =
    if Option.is_some (find_trait "http" op.dtraits) then []
    else
      List.concat_map
        (fun name ->
          List.map
            (fun (tr : Ast.trait) ->
              err Error_codes.protocol_trait_invalid tr.tspan
                "@%s is a protocol trait and requires @http on the operation"
                name)
            (traits_named name op.dtraits))
        [ "header"; "query"; "body" ]
  in
  protocol_http_diags @ check_header_shapes op @ check_query_shapes op
  @ check_body_shapes ctx op @ check_code op @ ref_diags @ endpoint_diags
  @ timeout_retry_diags

(* A value position (path, endpoint) that accepts the unified grammar:
   literal, template, or pure reference. Resolution of the refs a template or
   a pure reference carries is handled generically by [op_refs]/the caller's
   [ref_diags]; this only validates the position's own shape (template
   parses, no [{name}] input placeholder outside @http path — path is the one
   position that keeps it). *)
let value_position_diags ~allow_input (arg : Ast.trait_arg) span :
    Diagnostic.t list =
  match arg with
  | Ast.AString s ->
      let parts, diags = template_of ~span s in
      diags
      @
      if allow_input then []
      else
        List.filter_map
          (function
            | Ir.Tpl_input _ ->
                Some
                  (err Error_codes.protocol_trait_invalid span
                     "input placeholders ({name}) are only available in the \
                      @http path")
            | _ -> None)
          parts
  | Ast.ARef _ -> []
  | _ ->
      [
        err Error_codes.protocol_trait_invalid span
          "expected a string literal/template or a field reference";
      ]

let check_entry_op ctx (fields : Ast.member list) (op : Ast.decl) :
    Diagnostic.t list =
  let pname, pty = op_param op in
  let resolve segs = Entry_scope.resolve_ref ctx fields ~pname ~pty segs in
  (* A same-named entry field silently changes what `.name` means inside this
     op (parameter, not field). Not an error (ordinary lexical shadowing),
     but the only spot where one ref syntax has two possible resolutions. *)
  let shadow_diags =
    match pname with
    | Some p
      when List.exists (fun (m : Ast.member) -> String.equal m.mname p) fields
      ->
        [
          Diagnostic.warning ~code:Error_codes.param_shadows_field op.dname_span
            (Printf.sprintf
               "operation parameter '%s' shadows an entry field of the same \
                name; '.%s' refers to the parameter here"
               p p);
        ]
    | _ -> []
  in
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
            | Some (Entry_scope.RField m)
              when scalar_of_ty ctx m.mtype = Entry_scope.SString ->
                []
            | Some
                (Entry_scope.RParam
                   (Entry_scope.Whole (Ast.TPrim ("string", _)))) ->
                []
            | Some (Entry_scope.RParam (Entry_scope.Member m))
              when scalar_of_ty ctx m.mtype = Entry_scope.SString ->
                []
            | Some _ ->
                [
                  err Error_codes.entry_endpoint_missing r.ref_span
                    "endpoint '%s' must reference a string field"
                    (path_str r.segs);
                ]
            | None -> [] (* unknown ref already reported above *))
        | Some (Ast.AString _ as v) ->
            value_position_diags ~allow_input:false v http.tspan
        | Some _ ->
            [
              err Error_codes.entry_endpoint_missing http.tspan
                "endpoint: takes a literal, a template, or a field reference, \
                 not a bare literal of another kind";
            ])
  in
  let typed_ref name want scalar_desc =
    List.concat_map
      (fun (tr : Ast.trait) ->
        match tr.Ast.targs with
        | [ Ast.ARef r ] -> (
            match resolve r.segs with
            | None -> [] (* already reported *)
            | Some (Entry_scope.RField m) ->
                if want ctx m.mtype then []
                else
                  [
                    err Error_codes.protocol_trait_invalid r.ref_span
                      "@%s must reference a %s field" name scalar_desc;
                  ]
            | Some (Entry_scope.RParam _) ->
                [
                  err Error_codes.protocol_trait_invalid r.ref_span
                    "@%s must reference an entry field" name;
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
  let path_diags =
    match find_trait "http" op.dtraits with
    | Some { targs; tspan; _ } -> (
        match kv_arg "path" targs with
        | Some v -> value_position_diags ~allow_input:true v tspan
        | None -> [])
    | None -> []
  in
  check_protocol_positions op
  @ shadow_diags @ ref_diags @ http_diags @ path_diags @ timeout_diags
  @ retry_diags @ check_header_shapes op @ check_query_shapes op
  @ check_body_shapes ctx op @ check_code op
