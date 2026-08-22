(* The words the "ext" library block is made of, in one place.

   The block's declarations are the ordinary keywords ("ext", "struct",
   "op"); the only contextual words are the three lines of a language block,
   each written as `word:`. Everything specific to one target lives inside
   that target's block as a foreign spelling (#(...)), never as a word of the
   language, so this list is closed: a contract test pins it, and a new word
   fails that test rather than slipping in. *)

(* The lines of a language block, each written as `word:`. *)
let lang_fields = [ "call"; "yields"; "returns" ]

(* The reserved yields: position type, valid nowhere else. *)
let error_sentinel = "error"

(* The reference an extern call in a request trait may read: the assembled
   request. Its position rule lives in [Check_request_value]. *)
let request_ref = "request"

(* The traits an "ext" op accepts: the ones the rest of the language already
   has, no trait of its own. "async" lists the targets where the foreign
   call itself is asynchronous (absence means synchronous at the boundary);
   "errors" lists the declared error shapes the call can raise, in test
   order; "doc" documents it. *)
let op_traits = [ "async"; "errors"; "doc" ]

(* The targets that have an asynchronous call at all: naming any other one
   in @async is an error, not a no-op (Go has no await). *)
let async_targets = [ "ts"; "rust" ]

(* The targets an ext block can bind: the language names a block is headed
   by. *)
let targets = [ "go"; "ts"; "rust" ]
let quoted words = String.concat ", " (List.map (fun w -> "'" ^ w ^ "'") words)
