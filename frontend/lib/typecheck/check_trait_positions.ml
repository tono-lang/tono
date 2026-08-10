(* Known traits written in a position nothing reads (TC0069). A trait can be
   real and still be dropped: [check_trait_names] only tells a misspelling
   apart from a deliberate one, it does not know where a deliberate one is
   allowed.

   [Trait_vocab]'s groups are "who reads this", not "where it may be
   written": those are different axes, and [http] used to conflate them
   (@http reads the op, @httpResponseCode reads a member), so
   [Trait_vocab] now splits it into [http_op] and [http_member]. Position
   here is built from those two axis-correct lists plus the groups whose
   position already happens to be uniform:

   - constraints, member shape, entry sources, entry fields, and
     @httpResponseCode are read off a lowered member's own traits, never a
     decl's or a union variant's or an enum case's, so they are member-only
     everywhere.
   - @http and the per-request protocol knobs are read off an op's own
     traits, so they are legal there and nowhere else.

   The error taxonomy ([Trait_vocab.operations]) is deliberately left out:
   @errorCode/@status discriminate a declared error *shape*, referenced from
   an op's @errors list rather than written on the op itself, so the group
   has no single position to check against (see
   [entries_position_test.ml]'s "repeatable traits legal" and
   [declared_tests_test.ml]'s error struct for the legitimate shape-level
   form). Surface traits (@doc, @deprecated, @rename, @wire,
   @discriminator) describe the shape itself and stay legal everywhere. *)

let warn code span fmt = Printf.ksprintf (Diagnostic.warning ~code span) fmt

let member_only =
  Trait_vocab.constraints @ Trait_vocab.members @ Trait_vocab.sources
  @ Trait_vocab.entry_fields @ Trait_vocab.http_member

let op_only = Trait_vocab.http_op @ Trait_vocab.protocol

(* Already diagnosed with a more specific message in check_entries.ml: a
   construction source (TC0043) or @bind (TC0046) on a union variant or enum
   case. Skipped here so the same trait doesn't collect a second, less
   specific diagnostic. *)
let variant_or_case_exempt = Trait_vocab.sources @ [ "bind" ]

type site = Op | Shape | Member | Variant | Case

let describe = function
  | Op -> "an op"
  | Shape -> "a shape"
  | Member -> "a member"
  | Variant -> "a union variant"
  | Case -> "an enum case"

(* What is illegal to write at each site. [Member] only rejects [op_only]:
   [member_only] is its native home. [Variant]/[Case] reject both groups,
   minus the subset [check_entries] already covers. *)
let illegal_at = function
  | Op -> member_only
  | Shape -> member_only @ op_only
  | Member -> op_only
  | Variant | Case ->
      op_only
      @ List.filter
          (fun n -> not (List.mem n variant_or_case_exempt))
          member_only

let flag ~site (traits : Ast.trait list) : Diagnostic.t list =
  let illegal = illegal_at site in
  List.filter_map
    (fun (tr : Ast.trait) ->
      if List.mem tr.Ast.tname illegal then
        let home = if List.mem tr.Ast.tname op_only then Op else Member in
        Some
          (warn Error_codes.trait_position_invalid tr.tspan
             "@%s belongs on %s, not %s: nothing reads it here" tr.tname
             (describe home) (describe site))
      else None)
    traits

let rec check_decl (d : Ast.decl) : Diagnostic.t list =
  match d.dkind with
  | Ast.DOp _ -> flag ~site:Op d.dtraits
  | Ast.DStruct { members; ops; _ } ->
      flag ~site:Shape d.dtraits
      @ List.concat_map
          (fun (m : Ast.member) -> flag ~site:Member m.Ast.mtraits)
          members
      @ List.concat_map check_decl ops
  | Ast.DUnion { variants; _ } ->
      flag ~site:Shape d.dtraits
      @ List.concat_map
          (fun (v : Ast.union_variant) -> flag ~site:Variant v.Ast.vtraits)
          variants
  | Ast.DEnum { cases } ->
      flag ~site:Shape d.dtraits
      @ List.concat_map
          (fun (c : Ast.enum_case) -> flag ~site:Case c.Ast.ctraits)
          cases
  | Ast.DExt _ -> flag ~site:Shape d.dtraits
  | Ast.DTest _ -> []

let check_decls (decls : Ast.decl list) : Diagnostic.t list =
  List.concat_map check_decl decls
