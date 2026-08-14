(* Struct role classification. No keyword marks an entry or a config: the role
   emerges from content. A struct with ops in its body is an entry; a struct
   whose fields carry a construction source (@arg/@with/@env), or that an entry
   composes via @bind, is a config; everything else is wire data. @default alone
   does not classify: on a plain member it is the ordinary wire default. A
   foreign struct or opaque type declared inside an "ext" library block is
   always Foreign, regardless of its own content: it is a shape the checker
   only reads to validate a projection, never a shape a tono value can hold. *)

type role = Entry | Config | Wire | Foreign

let source_marker = function "arg" | "with" | "env" -> true | _ -> false

let member_has_source (m : Ast.member) : bool =
  List.exists (fun (t : Ast.trait) -> source_marker t.Ast.tname) m.Ast.mtraits

let member_has_bind (m : Ast.member) : bool =
  List.exists
    (fun (t : Ast.trait) -> String.equal t.Ast.tname "bind")
    m.Ast.mtraits

let classify (decls : Ast.decl list) : (string, role) Hashtbl.t =
  let roles = Hashtbl.create 16 in
  List.iter
    (fun (d : Ast.decl) ->
      match d.Ast.dkind with
      | Ast.DStruct { ops = _ :: _; _ } -> Hashtbl.replace roles d.dname Entry
      | Ast.DStruct { members; _ } when List.exists member_has_source members ->
          Hashtbl.replace roles d.dname Config
      | _ -> ())
    decls;
  (* Foreign names from every "ext" library block: foreign struct and opaque
     type names never belong to a real tono shape, so they classify Foreign
     regardless of their own field content. A name already claimed by a
     DStruct above is left alone here; that collision is a diagnostic owned
     by Check_ext_lib, not a silent overwrite. Opaque types are also
     registered under their qualified "ext_name.type_name" spelling, since
     that is how an entry field references an injectable handle
     (companybus.publisher); foreign structs stay bare, as they are only
     ever referenced unqualified inside their own block's language bodies. *)
  List.iter
    (fun (d : Ast.decl) ->
      match d.Ast.dkind with
      | Ast.DExtLib { body; _ } ->
          List.iter
            (fun (s : Ast.foreign_struct) ->
              if not (Hashtbl.mem roles s.Ast.fs_name) then
                Hashtbl.replace roles s.Ast.fs_name Foreign)
            body.Ast.elib_structs;
          List.iter
            (fun (t : Ast.opaque_type) ->
              if not (Hashtbl.mem roles t.Ast.opq_name) then
                Hashtbl.replace roles t.Ast.opq_name Foreign;
              let qualified = d.Ast.dname ^ "." ^ t.Ast.opq_name in
              if not (Hashtbl.mem roles qualified) then
                Hashtbl.replace roles qualified Foreign)
            body.Ast.elib_types
      | _ -> ())
    decls;
  (* A @bind composition marks its target type a config even when every value
     arrives by binding (the config then has no sources of its own). *)
  List.iter
    (fun (d : Ast.decl) ->
      match d.Ast.dkind with
      | Ast.DStruct { ops = _ :: _; members; _ } ->
          List.iter
            (fun (m : Ast.member) ->
              if member_has_bind m then
                (* Read through a nullable spelling: the field itself is
                   rejected elsewhere, but the classification must still flip
                   so the diagnostics describe the composition, not a
                   misplaced @bind. *)
                match m.Ast.mtype with
                | Ast.TName (n, [], _)
                | Ast.TNullable (Ast.TName (n, [], _), _)
                  when not (Hashtbl.mem roles n) ->
                    Hashtbl.replace roles n Config
                | _ -> ())
            members
      | _ -> ())
    decls;
  roles

let role_of (roles : (string, role) Hashtbl.t) (name : string) : role =
  Option.value ~default:Wire (Hashtbl.find_opt roles name)
