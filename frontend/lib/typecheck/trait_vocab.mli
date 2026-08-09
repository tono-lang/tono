(* The bare trait names the compiler acts on, collected from the checkers that
   read them so a misspelling can be told apart from a deliberate annotation.
   Namespaced traits such as @str::trim keep their own catalogs and are
   excluded here. *)

(* The groups themselves, exposed so a position checker can classify a decl
   trait by who reads it without a second hand-written list of names (see
   check_trait_positions.ml). *)
val constraints : string list
val members : string list
val sources : string list
val entry_fields : string list
val http : string list
val protocol : string list
val operations : string list
val surface : string list

(* Every known bare trait name. Editors render their vocabulary from this, so
   the compiler and the completion list cannot drift. *)
val known : string list
val is_known : string -> bool

(* Whether a trait belongs to a namespaced catalog (@str::trim), which this
   vocabulary does not cover. *)
val is_namespaced : string -> bool

(* The nearest known trait to a name, when one is close enough to suggest. *)
val nearest : string -> string option

(* Where a retired per-member HTTP binding trait's replacement lives, for the
   four names spelling distance would not connect to it. *)
val legacy_http_binding : string -> string option
