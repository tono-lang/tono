(* Struct role classification: roles emerge from content, never from keywords.
   Entry = struct with ops in the body; Config = struct with construction
   sources on its fields or composed by an entry via @bind; Wire = plain data. *)

type role = Entry | Config | Wire

(* Whether a trait name marks a construction source that classifies a struct
   (@arg/@with/@env; @default alone stays a wire default). *)
val source_marker : string -> bool

(* Whether the member carries any construction-source trait. *)
val member_has_source : Ast.member -> bool

(* Whether the member carries a @bind composition trait. *)
val member_has_bind : Ast.member -> bool

(* Classify every struct in the file; structs absent from the table are wire. *)
val classify : Ast.decl list -> (string, role) Hashtbl.t
val role_of : (string, role) Hashtbl.t -> string -> role
