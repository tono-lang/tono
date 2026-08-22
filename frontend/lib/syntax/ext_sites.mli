(* The source span of every foreign binding in a file, for a tool that
   checks a binding against the real library and needs to point at the
   [.tono] line that declared it rather than at generated code. Sites are
   keyed the way the IR names things (ext, language, owner, name), so a
   reader joins them against the compiled model without spans ever entering
   the IR. *)

type kind =
  | Path  (** the per-language module path: [go { #(path) }] *)
  | Handle  (** an opaque handle's storage block *)
  | Struct  (** a foreign form's language block *)
  | Op  (** a free op's [call:] line *)
  | Method  (** a handle method's [call:] line ([owner] is the handle) *)

type site = {
  ext : string;
  lang : string;
  kind : kind;
  owner : string option;
  name : string option;
  span : Span.span;
}

val kind_to_string : kind -> string
val of_file : Ast.file -> site list

(* One JSON object per site, the span rendered as [Span.to_string]. *)
val to_json : site -> Yojson.Safe.t
