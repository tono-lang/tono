(* Operation error references and error-discrimination traits. Each @errors
   entry on an operation names an error shape the operation may return and must
   resolve to a declared shape (TC0014). A declared error is discriminated by
   its HTTP status plus an optional body discriminator, so the referenced shape
   must carry a valid @status (TC0015) and any @errorCode must be a path and a
   value, both strings (TC0016); two declared errors of one operation must not
   share the same (status, path, value), or the mapping from a response to a
   type would be ambiguous (TC0017). Two coded errors sharing a status but
   reading different paths are legal (each guard probes its own location) but
   suspicious enough to warn (TC0067). @async is a bare marker and takes no
   arguments (TC0018). Non-name @errors arguments are rejected during lowering
   and never reach this list. *)

let err code span fmt = Printf.ksprintf (Diagnostic.error ~code span) fmt

let decl_by_name (decls : Ast.decl list) (name : string) : Ast.decl option =
  (* The first definition wins, matching [Symtab.build]. *)
  List.find_opt (fun (d : Ast.decl) -> String.equal d.dname name) decls

let find_trait name (traits : Ast.trait list) : Ast.trait option =
  List.find_opt (fun (t : Ast.trait) -> String.equal t.tname name) traits

(* The declared HTTP status of an error shape: [Ok (Some n)] for a well-formed
   @status(n), [Ok None] when the trait is absent, [Error ()] when it is
   present but its argument is not a bare integer. *)
let status_of (d : Ast.decl) : (int option, unit) result =
  match find_trait "status" d.dtraits with
  | None -> Ok None
  | Some { Ast.targs = [ Ast.AInt n ]; _ } -> Ok (Some n)
  | Some _ -> Error ()

(* The declared body discriminator of an error shape: the dotted path to probe
   (split into segments) and the value to match, or [Ok None] when the shape
   discriminates by status alone. *)
let code_of (d : Ast.decl) : ((string list * string) option, unit) result =
  match find_trait "errorCode" d.dtraits with
  | None -> Ok None
  | Some { Ast.targs = [ Ast.AString path; Ast.AString value ]; _ } ->
      Ok (Some (String.split_on_char '.' path, value))
  | Some _ -> Error ()

(* The distinct declared-error names of an operation, in declaration order.
   Repeats collapse so a doubly-listed error is not misreported as an ambiguous
   pair with itself. *)
let declared_error_names (d : Ast.decl) : string list =
  let names =
    List.concat_map
      (fun (tr : Ast.trait) ->
        if String.equal tr.tname "errors" then
          List.filter_map
            (function Ast.AName n -> Some n | _ -> None)
            tr.targs
        else [])
      d.dtraits
  in
  List.fold_left
    (fun acc n -> if List.mem n acc then acc else acc @ [ n ])
    [] names

(* Check one resolved declared error's discrimination traits, returning the
   diagnostics plus its (status, code) key when both are well-formed. *)
let check_declared_error (decls : Ast.decl list) (op : Ast.decl) (name : string)
    : Diagnostic.t list * (int * (string list * string) option) option =
  match decl_by_name decls name with
  | None -> ([], None) (* unresolved: already reported as TC0014 *)
  | Some target -> (
      let status_diags, status =
        match status_of target with
        | Ok (Some n) -> ([], Some n)
        | Ok None ->
            ( [
                err Error_codes.error_status_missing target.dname_span
                  "error type '%s' (declared by operation '%s') must carry \
                   @status with an integer HTTP status"
                  name op.dname;
              ],
              None )
        | Error () ->
            ( [
                err Error_codes.error_status_missing target.dname_span
                  "@status on '%s' expects a single integer argument, e.g. \
                   @status(404)"
                  name;
              ],
              None )
      in
      match code_of target with
      | Ok code -> (status_diags, Option.map (fun s -> (s, code)) status)
      | Error () ->
          ( status_diags
            @ [
                err Error_codes.error_code_invalid target.dname_span
                  "@errorCode on '%s' expects a path and a value, both \
                   strings, e.g. @errorCode(\"code\", \"not_found\")"
                  name;
              ],
            None ))

let code_desc = function
  | Some (path, value) ->
      Printf.sprintf "@errorCode(\"%s\", \"%s\")" (String.concat "." path) value
  | None -> "no @errorCode"

(* Two declared errors of one operation sharing a (status, path, value) triple
   cannot be told apart by the response, so it must be unique within the op. *)
let check_ambiguity (op : Ast.decl)
    (keys : (string * (int * (string list * string) option)) list) :
    Diagnostic.t list =
  let rec dups seen = function
    | [] -> []
    | (name, key) :: rest -> (
        match List.assoc_opt key seen with
        | Some first ->
            let status, code = key in
            err Error_codes.error_discrimination_ambiguous op.dname_span
              "operation '%s' cannot discriminate between errors '%s' and \
               '%s': both use @status(%d) with %s"
              op.dname first name status (code_desc code)
            :: dups seen rest
        | None -> dups ((key, name) :: seen) rest)
  in
  dups [] keys

(* Two coded errors sharing a status but probing different paths are legal
   (each guard reads its own location) but likely a mistake: warn once per
   operation rather than reject, since a real API can genuinely vary its
   discriminator's location by error. *)
let check_divergent_paths (op : Ast.decl)
    (keys : (string * (int * (string list * string) option)) list) :
    Diagnostic.t list =
  let coded =
    List.filter_map
      (fun (name, (status, code)) ->
        match code with
        | Some (path, _) -> Some (status, name, path)
        | None -> None)
      keys
  in
  let by_status =
    List.fold_left
      (fun acc (status, name, path) ->
        let existing = try List.assoc status acc with Not_found -> [] in
        (status, (name, path) :: existing) :: List.remove_assoc status acc)
      [] coded
  in
  List.concat_map
    (fun (status, entries) ->
      let paths = List.sort_uniq compare (List.map snd entries) in
      if List.length paths > 1 then
        [
          Diagnostic.warning ~code:Error_codes.error_code_paths_diverge
            op.dname_span
            (Printf.sprintf
               "operation '%s': declared errors at @status(%d) probe different \
                @errorCode paths (%s); confirm this API varies the \
                discriminator's location by error"
               op.dname status
               (String.concat ", "
                  (List.map (fun p -> String.concat "." p) paths)));
        ]
      else [])
    by_status

let check_op (tbl : Symtab.t) (decls : Ast.decl list) (op : Ast.decl) :
    Diagnostic.t list =
  let unresolved =
    List.concat_map
      (fun (tr : Ast.trait) ->
        if String.equal tr.tname "errors" then
          List.filter_map
            (function
              | Ast.AName name when Option.is_none (Symtab.find name tbl) ->
                  Some
                    (err Error_codes.unresolved_operation_ref tr.tspan
                       "unresolved error type '%s'" name)
              | _ -> None)
            tr.targs
        else [])
      op.dtraits
  in
  let async_diags =
    match find_trait "async" op.dtraits with
    | Some { Ast.targs = _ :: _; tspan; _ } ->
        [
          err Error_codes.async_takes_no_arguments tspan
            "@async takes no arguments";
        ]
    | _ -> []
  in
  let checked =
    List.map
      (fun name ->
        let diags, key = check_declared_error decls op name in
        (diags, Option.map (fun k -> (name, k)) key))
      (declared_error_names op)
  in
  let error_diags = List.concat_map fst checked in
  let keys = List.filter_map snd checked in
  unresolved @ async_diags @ error_diags @ check_ambiguity op keys
  @ check_divergent_paths op keys

let check_decl (tbl : Symtab.t) (decls : Ast.decl list) (d : Ast.decl) :
    Diagnostic.t list =
  match d.dkind with
  | Ast.DOp _ -> check_op tbl decls d
  (* Ops nested in an entry body obey the same error/async rules. *)
  | Ast.DStruct { ops; _ } -> List.concat_map (check_op tbl decls) ops
  | Ast.DEnum _ | Ast.DUnion _ | Ast.DExt _ | Ast.DTest _ -> []

let check_decls (tbl : Symtab.t) (decls : Ast.decl list) : Diagnostic.t list =
  List.concat_map (check_decl tbl decls) decls
