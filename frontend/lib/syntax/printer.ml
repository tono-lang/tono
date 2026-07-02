(* Pretty-printer for the surface AST. One canonical layout: declaration traits
   on their own lines above the keyword (except for ops, whose traits trail on
   the op line, where the grammar attaches them anyway), two-space indentation,
   one member, case, or variant per line, and a blank line between declarations.
   The printer assumes a diagnostic-free parse; [TError] gets a parseable
   placeholder only so a defensive caller never emits garbage. *)

let string_literal (s : string) : string =
  let b = Buffer.create (String.length s + 2) in
  Buffer.add_char b '"';
  String.iter
    (fun c ->
      match c with
      | '"' -> Buffer.add_string b "\\\""
      | '\\' -> Buffer.add_string b "\\\\"
      | '\n' -> Buffer.add_string b "\\n"
      | '\t' -> Buffer.add_string b "\\t"
      | '\r' -> Buffer.add_string b "\\r"
      | c -> Buffer.add_char b c)
    s;
  Buffer.add_char b '"';
  Buffer.contents b

(* Expand "1.5e+300" to positional notation: the literal grammar has no
   exponent form, so the digits are shifted around the decimal point. *)
let expand_exponent (s : string) : string =
  let e =
    match String.index_opt s 'e' with Some i -> i | None -> String.index s 'E'
  in
  let mant = String.sub s 0 e in
  let exp = int_of_string (String.sub s (e + 1) (String.length s - e - 1)) in
  let sign, mant =
    if mant.[0] = '-' then ("-", String.sub mant 1 (String.length mant - 1))
    else ("", mant)
  in
  let int_part, frac =
    match String.index_opt mant '.' with
    | Some d ->
        ( String.sub mant 0 d,
          String.sub mant (d + 1) (String.length mant - d - 1) )
    | None -> (mant, "")
  in
  let digits = int_part ^ frac in
  let point = String.length int_part + exp in
  let n = String.length digits in
  if point <= 0 then sign ^ "0." ^ String.make (-point) '0' ^ digits
  else if point >= n then sign ^ digits ^ String.make (point - n) '0' ^ ".0"
  else
    sign ^ String.sub digits 0 point ^ "." ^ String.sub digits point (n - point)

let float_literal (f : float) : string =
  if f <> f then "0.0" (* nan: no literal lexes to it, defensive only *)
  else if f = infinity || f = neg_infinity then
    (* An overflowing literal is the only positional spelling of infinity,
       mirroring how the lexer produced it. *)
    (if f < 0.0 then "-1" else "1") ^ String.make 309 '0' ^ ".0"
  else
    let s = ref (Printf.sprintf "%.17g" f) in
    (try
       for p = 1 to 17 do
         let c = Printf.sprintf "%.*g" p f in
         if float_of_string c = f then (
           s := c;
           raise Exit)
       done
     with Exit -> ());
    let s = !s in
    if String.contains s 'e' || String.contains s 'E' then expand_exponent s
    else if String.contains s '.' then s
    else s ^ ".0"

let rec print_ty (t : Ast.ty) : string =
  match t with
  | Ast.TPrim (p, _) -> p
  | Ast.TName (n, [], _) -> n
  | Ast.TName (n, args, _) ->
      n ^ "[" ^ String.concat ", " (List.map print_ty args) ^ "]"
  | Ast.TList (elem, _) -> "[]" ^ print_ty elem
  | Ast.TMap (k, v, _) -> "map[" ^ print_ty k ^ "]" ^ print_ty v
  | Ast.TNullable (inner, _) -> print_ty inner ^ "?"
  | Ast.TError _ -> "_"

let rec print_trait_arg (a : Ast.trait_arg) : string =
  match a with
  | Ast.AString s -> string_literal s
  | Ast.AInt n -> string_of_int n
  | Ast.AFloat f -> float_literal f
  | Ast.AName n -> n
  | Ast.AKv (k, v) -> k ^ ": " ^ print_trait_arg v

let print_trait (t : Ast.trait) : string =
  "@" ^ t.Ast.tname
  ^
  match t.Ast.targs with
  | [] -> ""
  | args -> "(" ^ String.concat ", " (List.map print_trait_arg args) ^ ")"

(* Traits appended to a line: members, cases, variants, and op declarations. *)
let trailing_traits (ts : Ast.trait list) : string =
  String.concat "" (List.map (fun t -> " " ^ print_trait t) ts)

let print_params = function [] -> "" | ps -> "[" ^ String.concat ", " ps ^ "]"

let print_member (m : Ast.member) : string =
  "  " ^ m.Ast.mname ^ ": " ^ print_ty m.Ast.mtype
  ^ trailing_traits m.Ast.mtraits

let print_enum_case (c : Ast.enum_case) : string =
  "  " ^ c.Ast.cname
  ^ (match c.Ast.cint with Some n -> " = " ^ string_of_int n | None -> "")
  ^ trailing_traits c.Ast.ctraits

let print_variant (v : Ast.union_variant) : string =
  "  " ^ v.Ast.vname
  ^ (match v.Ast.vpayload with Some t -> "(" ^ print_ty t ^ ")" | None -> "")
  ^ trailing_traits v.Ast.vtraits

(* header + a braced body, "{}" when empty *)
let braced (header : string) (lines : string list) : string =
  match lines with
  | [] -> header ^ " {}"
  | ls -> header ^ " {\n" ^ String.concat "\n" ls ^ "\n}"

let print_decl (d : Ast.decl) : string =
  let pub = if d.Ast.pub then "pub " else "" in
  match d.Ast.dkind with
  | Ast.DOp { input; output } ->
      (* Op traits print trailing: whitespace is not significant, so any trait
         between an op and the next declaration binds to the op regardless. *)
      pub ^ "op " ^ d.Ast.dname ^ "("
      ^ (match input with Some t -> print_ty t | None -> "")
      ^ ")"
      ^ (match output with Some t -> ": " ^ print_ty t | None -> "")
      ^ trailing_traits d.Ast.dtraits
  | kind ->
      let above =
        String.concat ""
          (List.map (fun t -> print_trait t ^ "\n") d.Ast.dtraits)
      in
      let body =
        match kind with
        | Ast.DStruct { params; members } ->
            braced
              (pub ^ "struct " ^ d.Ast.dname ^ print_params params)
              (List.map print_member members)
        | Ast.DEnum { cases } ->
            braced
              (pub ^ "enum " ^ d.Ast.dname)
              (List.map print_enum_case cases)
        | Ast.DUnion { params; variants } ->
            braced
              (pub ^ "union " ^ d.Ast.dname ^ print_params params)
              (List.map print_variant variants)
        | Ast.DOp _ -> assert false
      in
      above ^ body

let print_file (f : Ast.file) : string =
  match f with
  | [] -> ""
  | ds -> String.concat "\n\n" (List.map print_decl ds) ^ "\n"
