(* The scope an entry (or config) resolves against: role lookups, field-path
   resolution, scalar classification for match subjects, collection of the
   field references traits and operations consume, and the resolution graph
   over those references (cycles and lazy chains). *)

type ctx = { decls : Ast.decl list; roles : (string, Roles.role) Hashtbl.t }

(* The type under an optional [?]; entry/config rules read through it. *)
val base_ty : Ast.ty -> Ast.ty

(* The construction-source traits (@arg/@with/@env/@default) of a member. *)
val source_traits : Ast.member -> Ast.trait list
val decl_by_name : ctx -> string -> Ast.decl option
val struct_members : ctx -> string -> Ast.member list option
val role_of_name : ctx -> string -> Roles.role

(* Resolve a reference path against [fields]: the head names a field, further
   segments descend into struct-typed fields. *)
val resolve_path : ctx -> Ast.member list -> string list -> Ast.member option
val path_str : string list -> string

(* The scalar shapes a match subject can take; [SEnum] carries the case names. *)
type scalar = SBool | SString | SInt | SEnum of string list | SOther

val scalar_of_ty : ctx -> Ast.ty -> scalar

(* Field references inside one template string ({.a.b} placeholders). *)
val template_refs : string -> string list list

(* References consumed by a member's traits (env refs, format placeholders,
   bind sources); match refs are reported by [Check_entry_match] instead. *)
val member_trait_refs : Ast.member -> (string list * Span.span) list

(* All references a member consumes, match subject and arms included. *)
val member_refs : Ast.member -> (string list * Span.span) list
val protocol_trait_names : string list

(* References consumed by an operation's protocol traits. *)
val op_refs : Ast.decl -> (string list * Span.span) list

(* Whether a field declares any way to get a value on its own: sources, a
   match, a @format, or being a composed config. *)
val has_own_source : ctx -> Ast.member -> bool

(* Resolvable dependency heads of a field's references. *)
val dep_heads : ctx -> Ast.member list -> Ast.member -> string list

(* Cycle detection over the resolution graph (TC0039). *)
val check_cycles : ctx -> Ast.member list -> Diagnostic.t list

(* [broken ctx fields name] is the chain from [name] down to the first
   dependency with no declared source, or [None] when it statically resolves. *)
val broken : ctx -> Ast.member list -> string -> string list option

(* The consumption-point diagnostic naming a broken chain (TC0037). *)
val chain_error : Span.span -> string list -> string list -> Diagnostic.t
