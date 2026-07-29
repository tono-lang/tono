(* The bare trait names the compiler acts on, collected from the checkers that
   read them so a misspelling can be told apart from a deliberate annotation.
   Namespaced traits such as @str::trim keep their own catalogs and are
   excluded here. *)

(* Every known bare trait name. Editors render their vocabulary from this, so
   the compiler and the completion list cannot drift. *)
val known : string list
val is_known : string -> bool

(* Whether a trait belongs to a namespaced catalog (@str::trim), which this
   vocabulary does not cover. *)
val is_namespaced : string -> bool

(* The nearest known trait to a name, when one is close enough to suggest. *)
val nearest : string -> string option
