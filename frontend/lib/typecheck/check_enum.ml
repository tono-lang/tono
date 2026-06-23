(* Enum validation over the surface AST: value names are unique (TC0008), any
   explicit int values are unique (TC0008), and the backing is consistent
   (TC0009). An enum is int-backed when any case carries an explicit [= value];
   then every case must, otherwise its backing is ambiguous. *)

let err code span fmt = Printf.ksprintf (Diagnostic.error ~code span) fmt

let check_enum (cases : Ast.enum_case list) : Diagnostic.t list =
  let int_backed =
    List.exists (fun (c : Ast.enum_case) -> c.cint <> None) cases
  in
  let _, _, diags =
    List.fold_left
      (fun (names, ints, diags) (c : Ast.enum_case) ->
        let dup_name =
          if List.mem c.cname names then
            [
              err Error_codes.enum_value_duplicate c.cname_span
                "duplicate enum value '%s'" c.cname;
            ]
          else []
        in
        let dup_int =
          match c.cint with
          | Some n when List.mem n ints ->
              [
                err Error_codes.enum_value_duplicate c.cname_span
                  "duplicate enum backing value %d" n;
              ]
          | _ -> []
        in
        let mismatch =
          if int_backed && c.cint = None then
            [
              err Error_codes.enum_backing_mismatch c.cname_span
                "enum case '%s' needs an explicit '= value' because the enum \
                 is int-backed"
                c.cname;
            ]
          else []
        in
        let ints' = match c.cint with Some n -> n :: ints | None -> ints in
        (c.cname :: names, ints', diags @ dup_name @ dup_int @ mismatch))
      ([], [], []) cases
  in
  diags

let check_decl (d : Ast.decl) : Diagnostic.t list =
  match d.dkind with
  | Ast.DEnum { cases } -> check_enum cases
  | Ast.DStruct _ | Ast.DUnion _ | Ast.DOp _ -> []

let check_decls (file : Ast.file) : Diagnostic.t list =
  List.concat_map check_decl file
