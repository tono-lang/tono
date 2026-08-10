(* The bare trait names the compiler acts on, collected from the checkers that
   read them so a misspelling can be told apart from a deliberate annotation.
   Namespaced traits such as @str::trim keep their own catalogs and are
   excluded here. *)

(* The groups themselves, exposed so a position checker can classify a trait
   by where it is legal without a second hand-written list of names (see
   check_trait_positions.ml). Grouped by who reads them, which is a
   different axis from where they may be written: [http_op] and
   [http_member] split what was one reader-group ([http]) into the op-scoped
   name and the member-scoped one, since @http and @httpResponseCode read
   different positions. [operations] and [surface] carry no position rule of
   their own (see check_trait_positions.ml) and so are not exposed. *)
val constraints : string list
val members : string list
val sources : string list
val entry_fields : string list
val http_op : string list
val http_member : string list
val protocol : string list

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
