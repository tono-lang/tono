(* The words the language is made of, outside the ext library block (which
   [Ext_lib_vocab] owns), in one place the editor can be checked against.

   The reserved words come straight from the lexer's table, so they cannot
   drift. The contextual words are identifiers the parser recognizes only by
   position (Parser_ext, Parser, Parser_tests, Parser_traits); nothing else
   enumerates them, so this list is kept by hand and a parser test exercises
   every entry in its position. "hook" is deliberately absent: the parser
   still reads it after "ext" so the checker can name the migration, but it is
   not a construct the language accepts. *)

let keywords : string list = List.map fst Token.keywords

let contextual : string list =
  [ "contract"; "constraint"; "impl"; "raw"; "match"; "stub"; "expect"; "null" ]

let constructs : string list = keywords @ contextual
let is_construct (word : string) : bool = List.mem word constructs
