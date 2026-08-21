open Tono_frontend

(* Trait ownership by layout: a trait on a line of its own belongs to the
   declaration or body item after it, at the top level and inside a body
   alike; a trait continuing a line belongs to that line. The parser is the
   only place this is decided, so the checks here read the surface AST. *)

let parse src =
  let file, diags = Parser.parse src in
  ( file,
    List.filter (fun (d : Diagnostic.t) -> d.severity = Diagnostic.Error) diags
  )

let names (ts : Ast.trait list) = List.map (fun (t : Ast.trait) -> t.tname) ts

let struct_items src =
  match parse src with
  | ( { Ast.decls = [ { Ast.dkind = Ast.DStruct { members; ops; _ }; _ } ]; _ },
      [] ) ->
      (members, ops)
  | _, diags ->
      Alcotest.failf "expected one clean struct, got %d diagnostics"
        (List.length diags)

let member_traits src name =
  let members, _ = struct_items src in
  match List.find_opt (fun (m : Ast.member) -> m.mname = name) members with
  | Some m -> names m.mtraits
  | None -> Alcotest.failf "member %s not found" name

let op_traits src name =
  let _, ops = struct_items src in
  match List.find_opt (fun (d : Ast.decl) -> d.dname = name) ops with
  | Some d -> names d.dtraits
  | None -> Alcotest.failf "op %s not found" name

(* ── Inside a struct body ──────────────────────────────────────────────── *)

let entry =
  {|pub struct client {
  endpoint: string @env("EP")
  @doc("reads")
  @errors(overloaded)
  op fetch(ref: note_ref): note
  @doc("a field")
  token: string @env("T")
  op ping() @doc("inline")
  @doc("after ping")
  op pong()
}|}

let own_line_trait_precedes_an_op () =
  Alcotest.(check (list string))
    "traits above the op belong to it" [ "doc"; "errors" ]
    (op_traits entry "fetch")

let inline_member_trait_stays_on_the_member () =
  Alcotest.(check (list string))
    "the inline @env is the member's, the own-line @doc is not" [ "env" ]
    (member_traits entry "endpoint")

let own_line_trait_precedes_a_member () =
  Alcotest.(check (list string))
    "own-line doc above a member, inline env after it" [ "doc"; "env" ]
    (member_traits entry "token")

let inline_op_trait_stays_on_the_op () =
  Alcotest.(check (list string))
    "an inline trait after the signature is the op's" [ "doc" ]
    (op_traits entry "ping");
  Alcotest.(check (list string))
    "the own-line trait after it belongs to the next op" [ "doc" ]
    (op_traits entry "pong")

(* The old accident: a trait meant for the field below can no longer land on
   the op above, even when that op carries no trait of its own. *)
let trait_above_a_member_never_binds_to_the_op_before () =
  let src =
    {|pub struct c {
  op o(): r
  @doc("for k")
  k: string @env("K")
}|}
  in
  Alcotest.(check (list string)) "op stays bare" [] (op_traits src "o");
  Alcotest.(check (list string))
    "the field gets its doc" [ "doc"; "env" ] (member_traits src "k")

(* A multi-line continuation of a member's traits is not a thing: the second
   line belongs to the next item. *)
let member_trait_continuation_belongs_to_the_next_item () =
  let src = {|struct s {
  a: i64 @range(min: 0)
  @doc("b")
  b: i64
}|} in
  Alcotest.(check (list string))
    "a keeps its line" [ "range" ] (member_traits src "a");
  Alcotest.(check (list string))
    "b takes the next line" [ "doc" ] (member_traits src "b")

(* An inline head trait before a member value, as before. *)
let head_trait_before_a_value_is_inline () =
  let src = {|struct s { x: string @with = ns.f(.x) @doc("v") }|} in
  Alcotest.(check (list string))
    "both sides of the value" [ "with"; "doc" ] (member_traits src "x")

(* ── Top level ─────────────────────────────────────────────────────────── *)

let top_level_decls src =
  match parse src with
  | { Ast.decls; _ }, [] ->
      List.map (fun (d : Ast.decl) -> (d.dname, names d.dtraits)) decls
  | _, diags ->
      Alcotest.failf "expected a clean file, got %d diagnostics"
        (List.length diags)

let own_line_trait_after_a_top_level_op () =
  Alcotest.(check (list (pair string (list string))))
    "the trait below the op belongs to the struct"
    [ ("o", [ "async" ]); ("s", [ "doc" ]) ]
    (top_level_decls "@async op o(): i64\n@doc(\"s\")\nstruct s { x: i64 }")

let inline_trait_after_a_top_level_op () =
  Alcotest.(check (list (pair string (list string))))
    "an inline trait after the signature is the op's"
    [ ("o", [ "async"; "doc" ]); ("s", []) ]
    (top_level_decls "@async op o(): i64 @doc(\"o\")\nstruct s { x: i64 }")

let first_trait_in_the_file_is_leading () =
  Alcotest.(check (list (pair string (list string))))
    "a trait at the start of the file precedes its declaration"
    [ ("s", [ "doc" ]) ]
    (top_level_decls "@doc(\"s\") struct s { x: i64 }")

(* ── Enum cases and union variants ─────────────────────────────────────── *)

let enum_case_traits () =
  match parse "enum e {\n  @doc(\"a\")\n  a\n  b @doc(\"b\")\n}" with
  | { Ast.decls = [ { Ast.dkind = Ast.DEnum { cases }; _ } ]; _ }, [] ->
      Alcotest.(check (list (pair string (list string))))
        "own-line and inline case traits"
        [ ("a", [ "doc" ]); ("b", [ "doc" ]) ]
        (List.map (fun (c : Ast.enum_case) -> (c.cname, names c.ctraits)) cases)
  | _ -> Alcotest.fail "expected one clean enum"

let union_variant_traits () =
  match
    parse
      "union u @discriminator(\"k\") {\n\
      \  @doc(\"a\")\n\
      \  a(x)\n\
      \  b(y) @doc(\"b\")\n\
       }"
  with
  | ( {
        Ast.decls = [ { Ast.dkind = Ast.DUnion { variants; _ }; dtraits; _ } ];
        _;
      },
      [] ) ->
      Alcotest.(check (list string))
        "the inline head trait is the union's" [ "discriminator" ]
        (names dtraits);
      Alcotest.(check (list (pair string (list string))))
        "own-line and inline variant traits"
        [ ("a", [ "doc" ]); ("b", [ "doc" ]) ]
        (List.map
           (fun (v : Ast.union_variant) -> (v.vname, names v.vtraits))
           variants)
  | _ -> Alcotest.fail "expected one clean union"

(* ── Nothing follows ───────────────────────────────────────────────────── *)

let dangling what src =
  let _, diags = parse src in
  Alcotest.(check bool)
    (what ^ ": a trait with no item after it is diagnosed")
    true
    (List.exists
       (fun (d : Diagnostic.t) ->
         let m = d.message in
         let n = String.length "after its traits" in
         let rec has i =
           i + n <= String.length m
           && (String.sub m i n = "after its traits" || has (i + 1))
         in
         has 0)
       diags)

let trait_with_no_item_after_it_is_diagnosed () =
  dangling "struct" "struct s {\n  a: i64\n  @doc(\"x\")\n}";
  dangling "enum" "enum e {\n  a\n  @doc(\"x\")\n}";
  dangling "union" "union u {\n  a(x)\n  @doc(\"x\")\n}"

(* ── The formatter ─────────────────────────────────────────────────────── *)

(* The printer puts op traits above the signature, which is exactly where the
   parser reads them from: formatting is the identity on its own output and
   preserves ownership. *)
let fmt_prints_op_traits_above_and_round_trips () =
  let src =
    {|pub struct client {
  endpoint: string @env("EP")

  @doc("reads")
  @errors(overloaded)
  op fetch(ref: note_ref): note

  @doc("next")
  op ping()
}
|}
  in
  let file, diags = Parser.parse src in
  Alcotest.(check int) "clean parse" 0 (List.length diags);
  Alcotest.(check string) "fixpoint" src (Printer.print_file file);
  Alcotest.(check (list string))
    "fetch" [ "doc"; "errors" ] (op_traits src "fetch");
  Alcotest.(check (list string)) "ping" [ "doc" ] (op_traits src "ping")

let () =
  Alcotest.run "trait_layout"
    [
      ( "body",
        [
          Alcotest.test_case "own-line trait precedes an op" `Quick
            own_line_trait_precedes_an_op;
          Alcotest.test_case "inline member trait stays" `Quick
            inline_member_trait_stays_on_the_member;
          Alcotest.test_case "own-line trait precedes a member" `Quick
            own_line_trait_precedes_a_member;
          Alcotest.test_case "inline op trait stays" `Quick
            inline_op_trait_stays_on_the_op;
          Alcotest.test_case "no binding to the op before" `Quick
            trait_above_a_member_never_binds_to_the_op_before;
          Alcotest.test_case "member continuation is the next item's" `Quick
            member_trait_continuation_belongs_to_the_next_item;
          Alcotest.test_case "head trait before a value" `Quick
            head_trait_before_a_value_is_inline;
        ] );
      ( "top level",
        [
          Alcotest.test_case "own-line trait after an op" `Quick
            own_line_trait_after_a_top_level_op;
          Alcotest.test_case "inline trait after an op" `Quick
            inline_trait_after_a_top_level_op;
          Alcotest.test_case "first trait in the file" `Quick
            first_trait_in_the_file_is_leading;
        ] );
      ( "cases and variants",
        [
          Alcotest.test_case "enum cases" `Quick enum_case_traits;
          Alcotest.test_case "union variants" `Quick union_variant_traits;
        ] );
      ( "dangling",
        [
          Alcotest.test_case "no item after the trait" `Quick
            trait_with_no_item_after_it_is_diagnosed;
        ] );
      ( "fmt",
        [
          Alcotest.test_case "op traits above, round trip" `Quick
            fmt_prints_op_traits_above_and_round_trips;
        ] );
    ]
