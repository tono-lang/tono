(* Struct role classification. No keyword marks an entry or a config: the role
   emerges from content. A struct with ops in its body is an entry; a struct
   whose fields carry a construction source (@arg/@with/@env), or that an entry
   composes via @bind, is a config; everything else is wire data. @default alone
   does not classify: on a plain member it is the ordinary wire default. *)

type role = Entry | Config | Wire

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
  (* A @bind composition marks its target type a config even when every value
     arrives by binding (the config then has no sources of its own). *)
  List.iter
    (fun (d : Ast.decl) ->
      match d.Ast.dkind with
      | Ast.DStruct { ops = _ :: _; members; _ } ->
          List.iter
            (fun (m : Ast.member) ->
              if member_has_bind m then
                match m.Ast.mtype with
                | Ast.TName (n, [], _) when not (Hashtbl.mem roles n) ->
                    Hashtbl.replace roles n Config
                | _ -> ())
            members
      | _ -> ())
    decls;
  roles

let role_of (roles : (string, role) Hashtbl.t) (name : string) : role =
  Option.value ~default:Wire (Hashtbl.find_opt roles name)
