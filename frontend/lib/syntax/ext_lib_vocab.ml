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
   target's convention. "new" opts the call into TypeScript's own
   class-construction syntax (`new Symbol(args)`); Go/Rust ignore it, the
   same way every target that has no equivalent convention ignores a marker
   it doesn't act on. *)
let lang_markers = [ "sync"; "infallible"; "ctx"; "new" ]

(* The bare markers of an opaque handle's own "type" header. "interface"
   declares the foreign type is abstract (a Go interface, held by value),
   not a concrete struct held by pointer: the two spell differently in Go
   and nothing about a foreign name says which one it is. *)
let type_markers = [ "interface" ]
let lang_body_words = lang_fields @ lang_markers

(* The bare marker of an extern parameter's own type: the caller passes a
   collection of values for it instead of one (Go's `opts ...Option`,
   Rust's `Vec<T>`, TypeScript's `T[]`), each language's own `call:`
   materializing it in its own idiom. *)
let param_markers = [ "variadic" ]

(* The reserved yields: position type, valid nowhere else. *)
let error_sentinel = "error"

(* The reference an extern call in a request trait may read: the assembled
   request. Its position rule lives in [Check_request_value]. *)
let request_ref = "request"
let quoted words = String.concat ", " (List.map (fun w -> "'" ^ w ^ "'") words)
