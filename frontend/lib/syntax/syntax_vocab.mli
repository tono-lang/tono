(* The construct vocabulary of the language outside the ext library block:
   the lexer's reserved words plus the words the parser recognizes by
   position. The editor's construct docs are checked against it. *)

(* The reserved words, from the lexer's own table. *)
val keywords : string list

(* The words the parser recognizes only in position (contract, constraint,
   impl, raw, match, stub, expect). *)
val contextual : string list

(* [keywords @ contextual]. *)
val constructs : string list
val is_construct : string -> bool
