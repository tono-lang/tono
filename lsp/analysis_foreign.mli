(* The cursor inside a foreign spelling [#(...)]: which (ext, language)
   pair it belongs to, which position of the grammar the spelling fills,
   and what the pair's index offers there. See analysis_foreign.ml. *)

type site = {
  ext : string;
  lang : string;
  position : Foreign_index.position;
  prefix : string; (* the spelling's bytes before the cursor *)
}

(* Whether byte [off] is inside a [Foreign] token at all, an ext's or
   not: tono's own names are never what a spelling wants. *)
val inside : toks:Tono_frontend.Token.t list -> int -> bool

(* The site at byte [off], when it is inside a [Foreign] token of an ext
   block; None anywhere else. *)
val site_at :
  text:string -> toks:Tono_frontend.Token.t list -> int -> site option

(* The library path an ext declares for a language. *)
val package_of :
  Tono_frontend.Ast.file -> ext:string -> lang:string -> string option

val completion :
  lookup:Foreign_index.lookup ->
  file:Tono_frontend.Ast.file ->
  site ->
  Lsp.Types.CompletionItem.t list

(* Hover on the language word that opens a block ([go {], [ts {], [rust
   {]) inside an ext: the word, the index's status as prose, the word's
   span. *)
val lang_hover :
  lookup:Foreign_index.lookup ->
  toks:Tono_frontend.Token.t list ->
  int ->
  (string * string option * Tono_frontend.Span.span) option
