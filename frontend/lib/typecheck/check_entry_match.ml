(* Match-selection validation: the subject must resolve to a scalar field
   (bool, string, integer, or enum), patterns are literals of that type,
   duplicates and arms after the wildcard are unreachable, and the table must
   be exhaustive (bool and enum by coverage, string and int by wildcard). *)

let err code span fmt = Printf.ksprintf (Diagnostic.error ~code span) fmt

(* Re-expose the scalar constructors so patterns below read unqualified. *)
type scalar = Entry_scope.scalar =
  | SBool
  | SString
  | SInt
  | SEnum of string list
  | SOther

let resolve_path = Entry_scope.resolve_path
let path_str = Entry_scope.path_str
let scalar_of_ty = Entry_scope.scalar_of_ty

let pattern_key : Ast.match_pattern -> string = function
  | Ast.PString s -> "s:" ^ s
  | Ast.PInt n -> "i:" ^ string_of_int n
  | Ast.PName n -> "n:" ^ n
  | Ast.PWildcard -> "_"

let check_match ctx (fields : Ast.member list) (m : Ast.member)
    (fm : Ast.field_match) : Diagnostic.t list =
  match resolve_path ctx fields fm.subject.segs with
  | None ->
      [
        err Error_codes.field_ref_unknown fm.subject.ref_span
          "unknown field '%s' as the match subject" (path_str fm.subject.segs);
      ]
  | Some subject -> (
      match scalar_of_ty ctx subject.mtype with
      | SOther ->
          [
            err Error_codes.match_invalid fm.subject.ref_span
              "match subject '%s' must be a bool, string, integer, or enum \
               field"
              (path_str fm.subject.segs);
          ]
      | scalar ->
          let pattern_diags (a : Ast.match_arm) =
            match (scalar, a.pat) with
            | _, Ast.PWildcard -> []
            | SBool, Ast.PName ("true" | "false") -> []
            | SBool, _ ->
                [
                  err Error_codes.match_invalid a.pat_span
                    "a bool match takes the patterns true, false, or '_'";
                ]
            | SString, Ast.PString _ -> []
            | SString, _ ->
                [
                  err Error_codes.match_invalid a.pat_span
                    "a string match takes quoted literal patterns or '_'";
                ]
            | SInt, Ast.PInt _ -> []
            | SInt, _ ->
                [
                  err Error_codes.match_invalid a.pat_span
                    "an integer match takes integer literal patterns or '_'";
                ]
            | SEnum cases, Ast.PName n when List.mem n cases -> []
            | SEnum _, Ast.PName n ->
                [
                  err Error_codes.match_invalid a.pat_span
                    "'%s' is not a case of the matched enum" n;
                ]
            | SEnum _, _ ->
                [
                  err Error_codes.match_invalid a.pat_span
                    "an enum match takes bare case names or '_'";
                ]
            | SOther, _ -> []
          in
          (* Arm values are typed against the FIELD the match feeds (the
             subject only selects): a ref must carry the field's type, and a
             literal must spell a value of it. String literals stay legal for
             the boundary-parsed primitives (duration and friends), exactly
             like @default. *)
          let field_scalar = Entry_scope.scalar_of_ty ctx m.mtype in
          let field_ty_str = Printer.print_ty (Entry_scope.base_ty m.mtype) in
          let value_diags (a : Ast.match_arm) =
            match a.value with
            | Ast.AVRef r -> (
                match resolve_path ctx fields r.segs with
                | Some target
                  when not
                         (String.equal
                            (Printer.print_ty
                               (Entry_scope.base_ty target.mtype))
                            field_ty_str) ->
                    [
                      err Error_codes.match_invalid r.ref_span
                        "arm value '%s' has type %s but the field is %s"
                        (path_str r.segs)
                        (Printer.print_ty (Entry_scope.base_ty target.mtype))
                        field_ty_str;
                    ]
                | Some _ -> []
                | None ->
                    [
                      err Error_codes.field_ref_unknown r.ref_span
                        "unknown field '%s' in a match arm" (path_str r.segs);
                    ])
            | Ast.AVSources traits ->
                List.concat_map
                  (fun (tr : Ast.trait) ->
                    if List.mem tr.Ast.tname [ "env"; "default" ] then
                      List.filter_map
                        (function
                          | Ast.ARef r -> (
                              match resolve_path ctx fields r.Ast.segs with
                              | Some _ -> None
                              | None ->
                                  Some
                                    (err Error_codes.field_ref_unknown tr.tspan
                                       "unknown field '%s' in a match arm \
                                        source"
                                       (path_str r.segs)))
                          | _ -> None)
                        tr.targs
                    else
                      [
                        err Error_codes.match_invalid tr.tspan
                          "a match arm only stacks @env/@default sources";
                      ])
                  traits
            | Ast.AVString _ -> (
                match field_scalar with
                | SString | SOther -> []
                | SInt | SBool | SEnum _ ->
                    [
                      err Error_codes.match_invalid a.value_span
                        "a string arm value does not fit the %s field"
                        field_ty_str;
                    ])
            | Ast.AVInt _ -> (
                match field_scalar with
                | SInt | SOther -> []
                | SString | SBool | SEnum _ ->
                    [
                      err Error_codes.match_invalid a.value_span
                        "an integer arm value does not fit the %s field"
                        field_ty_str;
                    ])
            | Ast.AVName ("true" | "false") -> (
                match field_scalar with
                | SBool -> []
                | _ ->
                    [
                      err Error_codes.match_invalid a.value_span
                        "a bool arm value does not fit the %s field"
                        field_ty_str;
                    ])
            | Ast.AVName n -> (
                match field_scalar with
                | SEnum cases when List.mem n cases -> []
                | SEnum _ ->
                    [
                      err Error_codes.match_invalid a.value_span
                        "'%s' is not a case of the field's enum" n;
                    ]
                | _ ->
                    [
                      err Error_codes.match_invalid a.value_span
                        "a bare name arm value is an enum case; the field is %s"
                        field_ty_str;
                    ])
          in
          let dup_and_reach_diags =
            let rec go seen wild = function
              | [] -> []
              | (a : Ast.match_arm) :: rest ->
                  let here =
                    if wild then
                      [
                        err Error_codes.match_invalid a.pat_span
                          "unreachable arm: it follows the '_' wildcard";
                      ]
                    else if List.mem (pattern_key a.pat) seen then
                      [
                        err Error_codes.match_invalid a.pat_span
                          "duplicate pattern in match";
                      ]
                    else []
                  in
                  here
                  @ go
                      (pattern_key a.pat :: seen)
                      (wild || a.pat = Ast.PWildcard)
                      rest
            in
            go [] false fm.arms
          in
          let exhaustive =
            List.exists
              (fun (a : Ast.match_arm) -> a.pat = Ast.PWildcard)
              fm.arms
            ||
            let covered =
              List.filter_map
                (fun (a : Ast.match_arm) ->
                  match a.pat with Ast.PName n -> Some n | _ -> None)
                fm.arms
            in
            match scalar with
            | SBool -> List.mem "true" covered && List.mem "false" covered
            | SEnum cases -> List.for_all (fun c -> List.mem c covered) cases
            | _ -> false
          in
          let exhaustive_diags =
            if exhaustive then []
            else
              [
                err Error_codes.match_not_exhaustive fm.match_span
                  "match on field '%s' is not exhaustive; add the missing \
                   cases or a '_' arm"
                  m.mname;
              ]
          in
          List.concat_map pattern_diags fm.arms
          @ List.concat_map value_diags fm.arms
          @ dup_and_reach_diags @ exhaustive_diags)
