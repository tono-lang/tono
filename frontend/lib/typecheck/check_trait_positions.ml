(* Known traits written in a position nothing reads (TC0069). A trait can be
   real and still be dropped: [check_trait_names] only tells a misspelling
   apart from a deliberate one, it does not know where a deliberate one is
   allowed. [Trait_vocab]'s groups already say who reads each trait, so this
   pass reuses them instead of a fourth hand-written list of names, for the
   two groups whose decl-level position is uniform:

   - constraints, member shape, entry sources, and entry fields are read off
     a lowered member's own traits, never a decl's, so they are member-only
     everywhere.
   - the HTTP binding and the per-request protocol knobs are read off an
     op's own traits, so they are legal there and nowhere else.

   The error taxonomy ([Trait_vocab.operations]) is deliberately left out:
   @errorCode/@status discriminate a declared error *shape*, referenced from
   an op's @errors list rather than written on the op itself, so the group
   has no single decl-level home to check against (see
   [entries_position_test.ml]'s "repeatable traits legal" and
   [declared_tests_test.ml]'s error struct for the legitimate shape-level
   form). Surface traits (@doc, @deprecated, @rename, @wire,
   @discriminator) describe the shape itself and stay legal everywhere. *)

let warn code span fmt = Printf.ksprintf (Diagnostic.warning ~code span) fmt

let member_only =
  Trait_vocab.constraints @ Trait_vocab.members @ Trait_vocab.sources
  @ Trait_vocab.entry_fields

let op_only = Trait_vocab.http @ Trait_vocab.protocol

let flag ~is_op (traits : Ast.trait list) : Diagnostic.t list =
  List.filter_map
    (fun (tr : Ast.trait) ->
      if List.mem tr.Ast.tname member_only then
        Some
          (warn Error_codes.trait_position_invalid tr.tspan
             "@%s belongs on a member, not a shape: nothing reads it here"
             tr.tname)
      else if (not is_op) && List.mem tr.Ast.tname op_only then
        Some
          (warn Error_codes.trait_position_invalid tr.tspan
             "@%s belongs on an op, not a shape: nothing reads it here" tr.tname)
      else None)
    traits

let rec check_decl (d : Ast.decl) : Diagnostic.t list =
  match d.dkind with
  | Ast.DOp _ -> flag ~is_op:true d.dtraits
  | Ast.DStruct { ops; _ } ->
      flag ~is_op:false d.dtraits @ List.concat_map check_decl ops
  | Ast.DUnion _ | Ast.DEnum _ | Ast.DExt _ -> flag ~is_op:false d.dtraits
  | Ast.DTest _ -> []

let check_decls (decls : Ast.decl list) : Diagnostic.t list =
  List.concat_map check_decl decls
