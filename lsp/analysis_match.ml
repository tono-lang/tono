(* The map-indexed match forms ("base[key]", the mandatory "null" arm, and
   "._" in an arm's value position): AST-driven hover and completion, split
   out of [Analysis] to keep that file under the line-count ceiling. None of
   these three forms names a fixed lexical position the way "match" itself
   does right after "=" (the subject index is a whole [field_reference],
   "null" is an ordinary [match_pattern_name], and "._" an ordinary
   [field_reference] naming "_"), so resolution goes through the AST's own
   spans rather than a token-position scan. *)

module Ast = Tono_frontend.Ast
module Span = Tono_frontend.Span
module Printer = Tono_frontend.Printer

let contains (s : Span.span) (off : int) : bool =
  off >= s.Span.start.offset && off <= s.Span.finish.offset

(* Every match table a struct/config member declares (its `= match ...`
   value). *)
let field_matches (file : Ast.file) : Ast.field_match list =
  List.concat_map
    (fun (d : Ast.decl) ->
      match d.Ast.dkind with
      | Ast.DStruct { members; _ } ->
          List.filter_map
            (fun (m : Ast.member) ->
              match m.Ast.mvalue with
              | Some (Ast.MMatch fm) -> Some fm
              | _ -> None)
            members
      | _ -> [])
    file.Ast.decls

(* Hover content at [off], as (code, prose, span) for the caller to render —
   the indexed subject itself, a "null" pattern, or a "._" arm value. *)
let hover_at (file : Ast.file) (off : int) :
    (string * string option * Span.span) option =
  List.find_map
    (fun (fm : Ast.field_match) ->
      match fm.Ast.subject.Ast.index with
      | Some idx when contains idx.Ast.ref_span off ->
          Some
            ( Printer.print_ref fm.Ast.subject,
              Hover_docs.match_form_doc "map_index",
              idx.Ast.ref_span )
      | _ ->
          List.find_map
            (fun (a : Ast.match_arm) ->
              match a.Ast.pat with
              | Ast.PNull when contains a.Ast.pat_span off ->
                  Some ("null", Hover_docs.construct_doc "null", a.Ast.pat_span)
              | _ -> (
                  match a.Ast.value with
                  | Ast.AVSubject span when contains span off ->
                      Some ("._", Hover_docs.match_form_doc "subject_ref", span)
                  | _ -> None))
            fm.Ast.arms)
    (field_matches file)

(* Whether [off] sits in the value position of a match arm whose subject is
   map-indexed (so the arm can spell "._"), for completion to offer "_"
   alongside the ordinary sibling fields. *)
let in_indexed_arm_value (file : Ast.file) (off : int) : bool =
  List.exists
    (fun (fm : Ast.field_match) ->
      Option.is_some fm.Ast.subject.Ast.index
      && List.exists
           (fun (a : Ast.match_arm) -> contains a.Ast.value_span off)
           fm.Ast.arms)
    (field_matches file)
