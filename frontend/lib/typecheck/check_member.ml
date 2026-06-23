(* Member-level validation over the surface AST. Nullability is two-state: a
   member is [required] unless written [T?]. Writing [@required] on a [T?] member
   asks for both states at once, which is a TC0007 conflict. *)

let is_required (tr : Ast.trait) = String.equal tr.tname "required"

let check_member (m : Ast.member) : Diagnostic.t list =
  match m.mtype with
  | Ast.TNullable _ when List.exists is_required m.mtraits ->
      [
        Diagnostic.error ~code:Error_codes.nullability_conflict m.mname_span
          (Printf.sprintf
             "@required on the nullable member '%s' is contradictory; drop the \
              '?' or the @required"
             m.mname);
      ]
  | _ -> []

let check_decl (d : Ast.decl) : Diagnostic.t list =
  match d.dkind with
  | Ast.DStruct { members; _ } -> List.concat_map check_member members
  | Ast.DEnum _ | Ast.DUnion _ | Ast.DOp _ -> []

let check_decls (file : Ast.file) : Diagnostic.t list =
  List.concat_map check_decl file
