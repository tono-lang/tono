(* Entry-model validation: closed entry/config/wire boundaries, declared value
   sources (dead sources, lazy resolution chains, cycles), match selection
   (shape and exhaustiveness), @bind composition points, and protocol trait
   positions on operations (endpoint refs, @header/@timeout/@retry). *)

(* The closed @str::* catalog. Exposed so tooling (hover, completion) offers
   exactly the transforms this checker accepts. *)
val str_transforms : string list
val check_decls : Ast.decl list -> Diagnostic.t list
