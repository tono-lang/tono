(* Duplicate non-repeatable traits (TC0047). Some traits mean one thing per
   declaration or member (@doc, @http, @timeout, ...); a second occurrence is
   almost always an absorption accident: whitespace is not significant, so a
   trait written between an op and the next item binds to the op even across
   lines. Repeatable vocabulary (@errors, @header, @env, @default, @str::*,
   @rename keyed by language) stays untouched. *)

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

(* [hint] is appended to the message where the absorption footgun applies
   (operations, whose traits trail the line). *)
let dups ?(hint = "") (traits : Ast.trait list) : Diagnostic.t list =
  let rec go seen = function
    | [] -> []
    | (tr : Ast.trait) :: rest ->
        let here =
          if List.mem tr.Ast.tname non_repeatable && List.mem tr.tname seen then
            [
              warn Error_codes.duplicate_trait tr.tspan
                "duplicate @%s: this trait is not repeatable%s" tr.tname hint;
            ]
          else []
        in
        here @ go (tr.tname :: seen) rest
  in
  go [] traits

let op_hint =
  " (a trait written between an op and the next item binds to the op, even \
   across lines)"

let rec check_decl (d : Ast.decl) : Diagnostic.t list =
  match d.dkind with
  | Ast.DOp _ -> dups ~hint:op_hint d.dtraits
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

let check_decls (decls : Ast.decl list) : Diagnostic.t list =
  List.concat_map check_decl decls
