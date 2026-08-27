(* Struct role classification: roles emerge from content, never from keywords.
   Entry = struct with ops in the body; Config = struct with construction
   sources on its fields or composed by an entry via @bind; Wire = plain data;
   Foreign = a struct/handle declared inside an "ext <name> { ... }" FFI
   library block (foreign structs and opaque types), never real tono data. *)

type role = Entry | Config | Wire | Foreign

(* Whether a trait name marks a construction source that classifies a struct
   (@arg/@with/@env; @default alone stays a wire default). *)
val source_marker : string -> bool

(* Whether the member carries any construction-source trait. *)
val member_has_source : Ast.member -> bool

(* Whether the member carries a @bind composition trait. *)
val member_has_bind : Ast.member -> bool

(* Classify every struct in the file, plus every foreign struct/opaque type
   name declared inside an "ext" library block (Foreign); names absent from
   the table are wire. *)
val classify : Ast.decl list -> (string, role) Hashtbl.t
val role_of : (string, role) Hashtbl.t -> string -> role

(* The names a call: line may pass as a class reference: the file's
   non-generic wire structs (never an entry, a config, or a foreign shape). *)
val class_structs : (string, role) Hashtbl.t -> Ast.decl list -> string list
