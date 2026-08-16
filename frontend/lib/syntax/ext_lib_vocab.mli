(* The contextual words of the "ext" library block, shared by the parser's
   diagnostics and the editor's hover/completion so neither drifts. *)

val block_words : string list
val lang_fields : string list
val lang_markers : string list
val lang_body_words : string list
val error_sentinel : string
val request_ref : string

(* Comma-separated, single-quoted, for diagnostics. *)
val quoted : string list -> string
