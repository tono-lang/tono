(* HTTP binding validation. The Protocol resolver ([Protocol_http]) materializes
   an operation's HTTP annotations into a wire descriptor but assumes they are
   well-formed; the malformed cases are reported here, at the AST level, where
   source spans still exist (the IR carries none). Four failures:

   - a {placeholder} in the @http path with no matching @httpLabel member, or an
     @httpLabel member with no matching placeholder (TC0019);
   - @httpPayload (the member is the whole body) together with an unmarked body
     member, or more than one @httpPayload (TC0020);
   - a map-typed member bound to the query string or a header, which has no
     defined multi-value serialization (TC0021);
   - a nullable @httpLabel member, which could leave its {placeholder} empty
     (TC0022).

   Only operations carrying @http are checked; a purely local operation has no
   HTTP surface. *)

let err code span fmt = Printf.ksprintf (Diagnostic.error ~code span) fmt

let find_trait name (traits : Ast.trait list) : Ast.trait option =
  List.find_opt (fun (t : Ast.trait) -> String.equal t.tname name) traits

let has_trait name (traits : Ast.trait list) : bool =
  Option.is_some (find_trait name traits)

(* The @http path string, if the trait carries a well-formed [path:] argument. *)
let http_path (op : Ast.decl) : string option =
  match find_trait "http" op.dtraits with
  | None -> None
  | Some { targs; _ } ->
      List.find_map
        (function Ast.AKv ("path", Ast.AString p) -> Some p | _ -> None)
        targs

(* The {name} placeholders in a path template, in order. An unterminated brace
   ends the scan (the rest cannot name a placeholder). *)
let placeholders (path : string) : string list =
  let n = String.length path in
  let rec go i acc =
    if i >= n then List.rev acc
    else if path.[i] = '{' then
      match String.index_from_opt path i '}' with
      | Some j -> go (j + 1) (String.sub path (i + 1) (j - i - 1) :: acc)
      | None -> List.rev acc
    else go (i + 1) acc
  in
  go 0 []

let decl_by_name (file : Ast.file) (name : string) : Ast.decl option =
  List.find_opt (fun (d : Ast.decl) -> String.equal d.dname name) file

(* The members of the operation's input structure, or [] when the input is
   absent or is not a plain struct reference (a primitive input has no members;
   an unresolved name is reported elsewhere). *)
let input_members (file : Ast.file) (op : Ast.decl) : Ast.member list =
  match op.dkind with
  | Ast.DOp { input = Some (Ast.TName (name, [], _)); _ } -> (
      match decl_by_name file name with
      | Some { dkind = Ast.DStruct { members; _ }; _ } -> members
      | _ -> [])
  | _ -> []

let rec is_map_type : Ast.ty -> bool = function
  | Ast.TMap _ -> true
  | Ast.TNullable (t, _) -> is_map_type t
  | _ -> false

(* A member is a body field when it carries no binding trait: it is the default,
   and the one that conflicts with @httpPayload. *)
let is_body (m : Ast.member) : bool =
  not
    (has_trait "httpLabel" m.mtraits
    || has_trait "httpQuery" m.mtraits
    || has_trait "httpHeader" m.mtraits
    || has_trait "httpPayload" m.mtraits)

let check_labels (op : Ast.decl) (members : Ast.member list)
    (placeholders : string list) : Diagnostic.t list =
  let label_members =
    List.filter
      (fun (m : Ast.member) -> has_trait "httpLabel" m.mtraits)
      members
  in
  let label_names = List.map (fun (m : Ast.member) -> m.mname) label_members in
  let unmatched_placeholders =
    List.filter_map
      (fun name ->
        if List.mem name label_names then None
        else
          Some
            (err Error_codes.http_label_unmatched op.dname_span
               "path placeholder '{%s}' in operation '%s' has no @httpLabel \
                member of that name"
               name op.dname))
      placeholders
  in
  let unmatched_labels =
    List.filter_map
      (fun (m : Ast.member) ->
        if List.mem m.mname placeholders then None
        else
          Some
            (err Error_codes.http_label_unmatched m.mname_span
               "@httpLabel member '%s' has no matching '{%s}' placeholder in \
                the @http path"
               m.mname m.mname))
      label_members
  in
  unmatched_placeholders @ unmatched_labels

let check_payload (members : Ast.member list) : Diagnostic.t list =
  let payload_members =
    List.filter
      (fun (m : Ast.member) -> has_trait "httpPayload" m.mtraits)
      members
  in
  match payload_members with
  | [] -> []
  | first :: rest ->
      let extra =
        List.map
          (fun (m : Ast.member) ->
            err Error_codes.http_payload_conflict m.mname_span
              "at most one @httpPayload member is allowed; '%s' is a second"
              m.mname)
          rest
      in
      let body_conflicts =
        List.filter_map
          (fun (m : Ast.member) ->
            if is_body m then
              Some
                (err Error_codes.http_payload_conflict m.mname_span
                   "member '%s' cannot share the body with the @httpPayload \
                    member '%s'; @httpPayload is the whole body"
                   m.mname first.mname)
            else None)
          members
      in
      extra @ body_conflicts

let is_nullable_type : Ast.ty -> bool = function
  | Ast.TNullable _ -> true
  | _ -> false

(* A path parameter must be present to fill its {placeholder}: a nullable
   @httpLabel member would leave a hole in the uri when absent. *)
let check_label_presence (members : Ast.member list) : Diagnostic.t list =
  List.filter_map
    (fun (m : Ast.member) ->
      if has_trait "httpLabel" m.mtraits && is_nullable_type m.mtype then
        Some
          (err Error_codes.http_label_nullable m.mname_span
             "@httpLabel member '%s' must not be nullable; a path parameter is \
              always required"
             m.mname)
      else None)
    members

let check_maps (members : Ast.member list) : Diagnostic.t list =
  List.filter_map
    (fun (m : Ast.member) ->
      if
        is_map_type m.mtype
        && (has_trait "httpQuery" m.mtraits || has_trait "httpHeader" m.mtraits)
      then
        Some
          (err Error_codes.http_map_binding m.mname_span
             "map member '%s' cannot bind to a query parameter or header"
             m.mname)
      else None)
    members

let check_op (file : Ast.file) (op : Ast.decl) : Diagnostic.t list =
  match http_path op with
  | None -> []
  | Some path ->
      let members = input_members file op in
      check_labels op members (placeholders path)
      @ check_label_presence members
      @ check_payload members @ check_maps members

let check_decls (file : Ast.file) : Diagnostic.t list =
  List.concat_map
    (fun (d : Ast.decl) ->
      match d.dkind with Ast.DOp _ -> check_op file d | _ -> [])
    file
