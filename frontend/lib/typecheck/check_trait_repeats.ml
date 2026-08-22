(* Duplicate non-repeatable traits (TC0047). Some traits mean one thing per
   declaration or member (@doc, @http, @timeout, ...); a second occurrence
   is an accident the single-winner reading would otherwise hide. Repeatable
   vocabulary (@errors, @header, @env, @default, @str::*, @rename keyed by
   language) stays untouched. *)

(* A warning, not an error: the duplicate was harmless before (a single
   occurrence won), so existing specs keep compiling while the accident
   surfaces. *)
let warn code span fmt = Printf.ksprintf (Diagnostic.warning ~code span) fmt

let non_repeatable =
  [
    "doc";
    "http";
    "async";
    "timeout";
    "retry";
    "format";
    "status";
    "errorCode";
    "discriminator";
    "deprecated";
    "retryable";
    "wire";
  ]

let dups (traits : Ast.trait list) : Diagnostic.t list =
  let rec go seen = function
    | [] -> []
    | (tr : Ast.trait) :: rest ->
        let here =
          if List.mem tr.Ast.tname non_repeatable && List.mem tr.tname seen then
            [
              warn Error_codes.duplicate_trait tr.tspan
                "duplicate @%s: this trait is not repeatable" tr.tname;
            ]
          else []
        in
        here @ go (tr.tname :: seen) rest
  in
  go [] traits

let rec check_decl (d : Ast.decl) : Diagnostic.t list =
  match d.dkind with
  | Ast.DOp _ -> dups d.dtraits
  | Ast.DStruct { members; ops; _ } ->
      dups d.dtraits
      @ List.concat_map (fun (m : Ast.member) -> dups m.Ast.mtraits) members
      @ List.concat_map check_decl ops
  | Ast.DUnion { variants; _ } ->
      dups d.dtraits
      @ List.concat_map
          (fun (v : Ast.union_variant) -> dups v.Ast.vtraits)
          variants
  | Ast.DEnum { cases } ->
      dups d.dtraits
      @ List.concat_map (fun (c : Ast.enum_case) -> dups c.Ast.ctraits) cases
  | Ast.DExt _ -> dups d.dtraits
  | Ast.DExtLib _ -> dups d.dtraits
  | Ast.DTest _ -> []

let check_decls (decls : Ast.decl list) : Diagnostic.t list =
  List.concat_map check_decl decls
