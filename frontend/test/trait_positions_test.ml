open Tono_frontend

(* Parse + lower a snippet, then run the typecheck pass directly, returning its
   diagnostics in isolation from lowering's own (which the helper discards). *)
let check src =
  let file, _ = Parser.parse src in
  let diags = ref [] in
  let m = Lower.lower_file ~module_name:"m" ~diags file in
  let _, tc = Typecheck.check_module ~file m in
  tc

let codes src = List.filter_map (fun (d : Diagnostic.t) -> d.code) (check src)

(* ── Trait position (TC0069) ───────────────────────────────────────────── *)

(* Filtered to TC0069: the snippets may raise unrelated diagnostics, and the
   claim here is only about position. *)
let position_codes src = List.filter (fun c -> c = "TC0069") (codes src)

(* A member-only trait written as the shape's own trait is a real name that
   nothing reads there: the constraint checker only ever walks a lowered
   member's constraints, so it never sees a decl-level @range. *)
let member_only_trait_on_a_decl_is_reported () =
  Alcotest.(check (list string))
    "@range on the struct itself" [ "TC0069" ]
    (codes {|@range(min: 0) struct s { a: i64 }|});
  Alcotest.(check (list string))
    "@required on the struct itself" [ "TC0069" ]
    (codes {|@required struct s { a: i64 }|});
  Alcotest.(check (list string))
    "@arg on the struct itself" [ "TC0069" ]
    (position_codes {|@arg struct s { a: string @arg }|});
  Alcotest.(check (list string))
    "@format on the struct itself" [ "TC0069" ]
    (position_codes {|@format("{x}") struct s { a: string @arg }|});
  (* @httpResponseCode reads a member's own traits (Protocol_http), not the
     op's: it belongs to the same reader-group as @http ([Trait_vocab.http]
     before the split) but the opposite position. *)
  Alcotest.(check (list string))
    "@httpResponseCode on the struct itself" [ "TC0069" ]
    (position_codes {|@httpResponseCode struct s { a: i64 }|});
  Alcotest.(check (list string))
    "@httpResponseCode on an op" [ "TC0069" ]
    (position_codes {|@httpResponseCode op o(): i64|})

(* An op-scoped trait written on a member is dropped the same way a
   member-scoped one on a shape is: the traversal used to stop at a decl's
   own traits, so this was silent even though nothing consumes @http off a
   member either. *)
let op_only_trait_on_a_member_is_reported () =
  Alcotest.(check (list string))
    "@http on a member" [ "TC0069" ]
    (position_codes {|struct s { a: i64 @http(method: "get", path: "/x") }|});
  Alcotest.(check (list string))
    "@retry on a member" [ "TC0069" ]
    (position_codes {|struct s { a: i64 @retry(3) }|})

(* @httpResponseCode is legal exactly where it is read: on a member. *)
let http_response_code_on_a_member_is_silent () =
  Alcotest.(check (list string))
    "@httpResponseCode on a member" []
    (position_codes {|struct s { a: i64 @httpResponseCode }|})

(* An op-only trait written on a plain struct or union is dropped the same
   way: the HTTP binding and the per-request protocol knobs read an op's own
   traits, never a struct's. (The error taxonomy is exempt: @status/@errorCode
   are legal on a declared error shape, not just an op; see
   check_trait_positions.ml.) *)
let op_only_trait_on_a_non_op_is_reported () =
  Alcotest.(check (list string))
    "@http on a union" [ "TC0069" ]
    (codes {|@http(method: "get", path: "/x") union u { bare }|});
  Alcotest.(check (list string))
    "@timeout on an enum" [ "TC0069" ]
    (codes {|@timeout(1000) enum e { x, y }|})

let op_traits_on_an_op_are_silent () =
  Alcotest.(check (list string))
    "http surface on an op" []
    (position_codes
       {|@http(method: "get", path: "/x/{id}") @async @retryable @errors([])
         op o(): i64|});
  Alcotest.(check (list string))
    "protocol knobs on an entry op" []
    (position_codes
       {|struct s {
           op fetch(): i64 @http(method: "get", path: "/x") @timeout(1000) @retry(3)
         }|})

let surface_traits_are_always_silent () =
  Alcotest.(check (list string))
    "doc, deprecated, rename, wire, discriminator on every decl kind" []
    (position_codes
       {|@doc("d") @deprecated @wire("s") struct s { a: i64 }
         @doc("u") @discriminator("kind") union un { bare }
         @doc("e") enum e { x, y }
         @doc("o") @rename("go") op o(): i64|})

(* A union variant or an enum case reads neither group: check_constraints and
   check_member only ever walk a struct's members, so both are silent
   sinks the same way a plain struct's own traits used to be. *)
let member_only_trait_on_a_variant_or_case_is_reported () =
  Alcotest.(check (list string))
    "@timeout on a union variant" [ "TC0069" ]
    (position_codes {|union u { bare(i64) @timeout(1000) }|});
  Alcotest.(check (list string))
    "@required on an enum case" [ "TC0069" ]
    (position_codes {|enum e { x @required, y }|})

(* @arg and @bind on a union variant or enum case are already reported by
   check_entries.ml (TC0043/TC0046) with a message that names the
   construction-boundary rule directly; TC0069 stays quiet rather than
   pile a second, less specific diagnostic onto the same trait. *)
let variant_and_case_exempt_traits_are_not_double_reported () =
  Alcotest.(check (list string))
    "@arg on a union variant" []
    (position_codes {|union u { bare(i64) @arg }|});
  Alcotest.(check (list string))
    "@bind on an enum case" []
    (position_codes {|enum e { x @bind("b"), y }|})

(* [Check_trait_positions.member_only] and [.op_only] must never share a
   name: a trait in both would make [illegal_at]'s answer depend on which
   branch happens to run first, silently misclassifying it the way
   @httpResponseCode was before the [http] group was split by position. *)
let position_groups_are_disjoint () =
  let overlap =
    List.filter
      (fun n -> List.mem n Check_trait_positions.op_only)
      Check_trait_positions.member_only
  in
  Alcotest.(check (list string)) "no trait in both position groups" [] overlap

(* [Check_trait_repeats.non_repeatable] classifies repeatability, a property
   the groups in [Trait_vocab] don't carry, so it stays its own hand-written
   list. This does not gate it against drift outright, but it does catch the
   half that would otherwise fail silently: a name here that fell out of the
   vocabulary (renamed or retired) would still compile, just stop meaning
   anything. *)
let non_repeatable_is_a_subset_of_the_vocabulary () =
  let unknown =
    List.filter
      (fun name -> not (Trait_vocab.is_known name))
      Check_trait_repeats.non_repeatable
  in
  Alcotest.(check (list string)) "every non-repeatable name is known" [] unknown

(* [Trait_vocab.known] is the concatenation of its groups; a name copy-pasted
   into two groups would silently double up in [known] without breaking
   anything at the point of the mistake, and [Check_trait_positions] would
   then apply the wrong position rule to it. *)
let known_has_no_duplicate_membership () =
  let seen = Hashtbl.create 32 in
  let dups = ref [] in
  List.iter
    (fun name ->
      if Hashtbl.mem seen name then dups := name :: !dups
      else Hashtbl.add seen name ())
    Trait_vocab.known;
  Alcotest.(check (list string)) "no trait in two groups" [] !dups

let () =
  Alcotest.run "trait-positions"
    [
      ( "trait-position",
        [
          Alcotest.test_case "member-only trait on a decl" `Quick
            member_only_trait_on_a_decl_is_reported;
          Alcotest.test_case "op-only trait on a member" `Quick
            op_only_trait_on_a_member_is_reported;
          Alcotest.test_case "httpResponseCode on a member is silent" `Quick
            http_response_code_on_a_member_is_silent;
          Alcotest.test_case "op-only trait on a non-op" `Quick
            op_only_trait_on_a_non_op_is_reported;
          Alcotest.test_case "op traits on an op are silent" `Quick
            op_traits_on_an_op_are_silent;
          Alcotest.test_case "surface traits always silent" `Quick
            surface_traits_are_always_silent;
          Alcotest.test_case "member-only trait on a variant or case" `Quick
            member_only_trait_on_a_variant_or_case_is_reported;
          Alcotest.test_case "variant/case exempt traits not double-reported"
            `Quick variant_and_case_exempt_traits_are_not_double_reported;
        ] );
      ( "vocabulary-drift",
        [
          Alcotest.test_case "position groups are disjoint" `Quick
            position_groups_are_disjoint;
          Alcotest.test_case "non-repeatable names are known" `Quick
            non_repeatable_is_a_subset_of_the_vocabulary;
          Alcotest.test_case "no trait in two groups" `Quick
            known_has_no_duplicate_membership;
        ] );
    ]
