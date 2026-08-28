(* The symbol index of one (ext, language) pair as `tono index` writes it,
   read from disk by the editor: what a foreign spelling can name, offered
   by the position the cursor is in. Pure: the server hands in the file's
   text and the values the key is checked against. See foreign_index.ml. *)

open Lsp.Types

type kind =
  | Function
  | Class
  | Struct
  | Interface
  | Type
  | Enum
  | Const
  | Namespace
  | Trait
  | Other

type member_kind =
  | Method
  | Field
  | Constructor
  | Member_function
  | Member_type
  | Member_const
  | Member_other

type member = {
  mname : string;
  mkind : member_kind;
  static : bool;
  msignatures : string list;
}

type symbol = {
  name : string;
  kind : kind;
  signatures : string list;
  doc : string;
  members : member list;
}

(* What the index was built from; the reader recomputes it and discards a
   file whose key differs. *)
type key = {
  ext : string;
  lang : string;
  package : string;
  version : string;
  lockfile_path : string;
  lockfile_digest : string;
  format : int;
}

type t = { key : key; note : string option; symbols : symbol list }

(* An index the editor can serve, or the reason it has none. *)
type status = Ready of t | Missing of string

(* What the server gives the analysis: the status of a pair's index. *)
type lookup = ext:string -> lang:string -> status

(* The index file format this reader understands. *)
val format : int
val of_string : string -> (t, string) result

(* FNV-1a over 64 bits, as 16 hex digits: the lockfile digest both sides
   compute. *)
val fnv1a64_hex : string -> string

(* The digest recorded for an absent lockfile. *)
val no_lockfile : string

(* The key an index built now would carry: [lockfile] is the file's bytes,
   [None] when it is absent. *)
val expected_key :
  ext:string ->
  lang:string ->
  package:string ->
  version:string ->
  lockfile_path:string ->
  lockfile:string option ->
  key

val key_matches : t -> key -> bool

(* Where inside a spelling the cursor is, which decides what is offered. *)
type position =
  | Call_head of { after_new : bool }
    (* the callee of a [call:] line: functions and classes; after
         [new ], classes alone *)
  | Type_pos (* a storage, form or argument type: the exported types *)
  | Function_pos (* a bare or chained call in argument position *)
  | Path (* the module path of the ext header: nothing to offer *)
  | Member of { head : string; base : position }
(* after [.] or [::]: the members of [head] *)

val symbol : t -> string -> symbol option
val items : t -> position -> CompletionItem.t list

(* The hover prose for a language block: what the index holds, or why
   there is none. *)
val describe : status -> string
