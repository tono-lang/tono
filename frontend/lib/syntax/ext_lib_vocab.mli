(* The closed vocabulary of the "ext" library block, shared by the parser's
   diagnostics and the editor's hover/completion so neither drifts. *)

val lang_fields : string list
val error_sentinel : string
val request_ref : string
val op_traits : string list
val async_targets : string list
val targets : string list

(* Comma-separated, single-quoted, for diagnostics. *)
val quoted : string list -> string
