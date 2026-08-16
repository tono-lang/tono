(* The words the "ext" library block is made of, in one place.

   Unlike struct/enum/union/op, none of these is a lexer keyword: they are
   contextual identifiers the parser recognizes by position (extern, type,
   call, ...). That is what let the editor fall out of step with the
   grammar once, so the vocabulary is named here and consumed by both the
   parser (its diagnostics enumerate it) and the editor (hover, completion,
   and a contract test that every word here is documented and accepted). *)

(* The declarations an ext body introduces by a contextual word ("struct" is
   a lexer keyword and is not listed). *)
let block_words = [ "extern"; "type" ]

(* The lines of a language block, each written as `word:`. *)
let lang_fields = [ "call"; "yields"; "returns"; "errors" ]

(* The bare markers of a language block, each opting a call out of one
   target's convention. *)
let lang_markers = [ "sync"; "infallible" ]
let lang_body_words = lang_fields @ lang_markers

(* The reserved yields: position type, valid nowhere else. *)
let error_sentinel = "error"

(* The reference an extern call in a request trait may read: the assembled
   request. Its position rule lives in [Check_request_value]. *)
let request_ref = "request"
let quoted words = String.concat ", " (List.map (fun w -> "'" ^ w ^ "'") words)
