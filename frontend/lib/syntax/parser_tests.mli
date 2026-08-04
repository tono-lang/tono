(* The test-block grammar ([test "name" { ... }]). Values replicate the
   calculus ctor/literal subset over the main token stream; patterns are the
   test's own grammar (the '..', 'any', and 'None' marks). *)

(* Parse a value (a literal, ctor, list, map, or binding reference). *)
val parse_value : Parser_state.t -> Ast.test_value

(* Parse an expect pattern. *)
val parse_pattern : Parser_state.t -> Ast.test_pattern

(* Parse a whole [test] declaration; the cursor is on the 'test' keyword. *)
val parse_test : Parser_state.t -> Ast.decl
