(* Lowering: surface AST to the IR. This is where the surface-to-IR mapping
   lives -- primitive keyword to [Prim], type-parameter use to [Param], generic
   application to [Ref] with args, list/map to their IR forms -- plus the
   policies (no [decimal], snake_case) that the parser stays free of. Lowering
   accumulates diagnostics into the shared sink rather than raising. *)

let prim_of_keyword : string -> Ir.prim = function
  | "bool" -> Ir.Bool
  | "string" -> Ir.String
  | "bytes" -> Ir.Bytes
  | "float" -> Ir.Float
  | "timestamp" -> Ir.Timestamp
  | "date" -> Ir.Date
  | "duration" -> Ir.Duration
  | "uuid" -> Ir.Uuid
  | "i8" -> Ir.int_prim ~bits:8 ~signed:true
  | "i16" -> Ir.int_prim ~bits:16 ~signed:true
  | "i32" -> Ir.int_prim ~bits:32 ~signed:true
  | "i64" -> Ir.int_prim ~bits:64 ~signed:true
  | "u8" -> Ir.int_prim ~bits:8 ~signed:false
  | "u16" -> Ir.int_prim ~bits:16 ~signed:false
  | "u32" -> Ir.int_prim ~bits:32 ~signed:false
  | "u64" -> Ir.int_prim ~bits:64 ~signed:false
  (* The lexer only ever produces the primitives above, so this is unreachable;
     a harmless default keeps lowering total and non-raising. *)
  | _ -> Ir.String

let report diags (d : Diagnostic.t) = diags := d :: !diags

(* [params] are the type-parameter names in scope (a shape's generic header), so
   a bare name that matches one lowers to [Param] rather than a named [Ref]. *)
let rec lower_type ~(params : string list) ~(diags : Diagnostic.t list ref)
    (t : Ast.ty) : Ir.tref =
  match t with
  | Ast.TPrim (kw, _) -> Ir.Prim (prim_of_keyword kw)
  | Ast.TName (name, [], span) ->
      if String.equal name "decimal" then (
        report diags
          (Diagnostic.error span
             "there is no 'decimal' type; model money as an integer of minor \
              units, or use 'float'");
        Ir.Ref (name, []))
      else if List.mem name params then Ir.Param name
      else Ir.Ref (name, [])
  | Ast.TName (name, args, _) ->
      Ir.Ref (name, List.map (lower_type ~params ~diags) args)
  | Ast.TList (inner, _) -> Ir.List (lower_type ~params ~diags inner)
  | Ast.TMap (k, v, _) ->
      Ir.Map (lower_type ~params ~diags k, lower_type ~params ~diags v)
  | Ast.TNullable (inner, _) ->
      (* Nullability is a member-level flag; at the type level the inner type is
         what reaches the IR. The member lowering reads the [?] separately. *)
      lower_type ~params ~diags inner
  | Ast.TError _ -> Ir.Ref ("", [])

(* ── Names ─────────────────────────────────────────────────────────────── *)

let is_snake_case (s : string) : bool =
  (not (String.equal s ""))
  && (not (s.[0] >= '0' && s.[0] <= '9'))
  && String.for_all
       (fun c -> (c >= 'a' && c <= 'z') || (c >= '0' && c <= '9') || c = '_')
       s

let check_snake diags span what name =
  if not (is_snake_case name) then
    report diags
      (Diagnostic.error span
         (Printf.sprintf "%s '%s' must be snake_case" what name))

(* ── Trait argument JSON ───────────────────────────────────────────────── *)

let rec json_of_arg : Ast.trait_arg -> Ir.json = function
  | Ast.AString s -> `String s
  | Ast.AInt n -> `Int n
  | Ast.AName s -> `String s
  | Ast.AKv (k, v) -> `Assoc [ (k, json_of_arg v) ]

let json_of_args : Ast.trait_arg list -> Ir.json = function
  | [] -> `Null
  | args -> `List (List.map json_of_arg args)

let bag_trait (tr : Ast.trait) : Ir.trait =
  { Ir.trait_id = "core#" ^ tr.tname; value = json_of_args tr.targs }

let lower_bag_traits trs = List.map bag_trait trs

(* ── Core constraint lifting ───────────────────────────────────────────── *)

let kv_arg key args =
  List.find_map
    (function Ast.AKv (k, v) when String.equal k key -> Some v | _ -> None)
    args

let int_of_arg = function Ast.AInt n -> Some n | _ -> None
let float_of_arg = function Ast.AInt n -> Some (float_of_int n) | _ -> None

(* Bounds accept either positional [min, max] or keyword [min:, max:] forms. *)
let range_bounds args =
  match args with
  | [ Ast.AInt a; Ast.AInt b ] -> (Some (float_of_int a), Some (float_of_int b))
  | _ ->
      ( Option.bind (kv_arg "min" args) float_of_arg,
        Option.bind (kv_arg "max" args) float_of_arg )

let length_bounds args =
  match args with
  | [ Ast.AInt a; Ast.AInt b ] -> (Some a, Some b)
  | _ ->
      ( Option.bind (kv_arg "min" args) int_of_arg,
        Option.bind (kv_arg "max" args) int_of_arg )

let constraint_of_trait diags (tr : Ast.trait) : Ir.constraint_ option =
  match tr.tname with
  | "range" ->
      let min, max = range_bounds tr.targs in
      Some (Ir.range ?min ?max ())
  | "length" ->
      let min, max = length_bounds tr.targs in
      Some (Ir.length ?min ?max ())
  | "pattern" -> (
      match tr.targs with
      | Ast.AString s :: _ -> Some (Ir.pattern s)
      | _ ->
          report diags
            (Diagnostic.error tr.tspan "@pattern expects a string argument");
          None)
  | "multipleOf" -> (
      match tr.targs with
      | Ast.AInt n :: _ -> Some (Ir.multiple_of (float_of_int n))
      | _ ->
          report diags
            (Diagnostic.error tr.tspan "@multipleOf expects an integer argument");
          None)
  | _ -> None

(* ── Members ───────────────────────────────────────────────────────────── *)

let lower_member ~params ~diags (m : Ast.member) : Ir.member =
  check_snake diags m.mname_span "member name" m.mname;
  let base, nullable =
    match m.mtype with Ast.TNullable (t, _) -> (t, true) | t -> (t, false)
  in
  let target = lower_type ~params ~diags base in
  let required = ref (not nullable) in
  let default = ref None in
  let constraints = ref [] in
  let bag = ref [] in
  List.iter
    (fun (tr : Ast.trait) ->
      match tr.tname with
      | "required" -> required := true
      | "default" ->
          let v = match tr.targs with a :: _ -> json_of_arg a | [] -> `Null in
          default := Some v
      | "range" | "length" | "pattern" | "multipleOf" -> (
          match constraint_of_trait diags tr with
          | Some c -> constraints := !constraints @ [ c ]
          | None -> ())
      | _ -> bag := !bag @ [ bag_trait tr ])
    m.mtraits;
  {
    Ir.name = m.mname;
    target;
    required = !required;
    default = !default;
    constraints = !constraints;
    traits = !bag;
  }

(* ── Declarations ──────────────────────────────────────────────────────── *)

let lower_decl ~diags (d : Ast.decl) : Ir.shape =
  check_snake diags d.dname_span "shape name" d.dname;
  let pub_trait =
    if d.pub then [ { Ir.trait_id = "core#pub"; value = `Null } ] else []
  in
  let traits = pub_trait @ lower_bag_traits d.dtraits in
  match d.dkind with
  | Ast.DStruct { params; members } ->
      {
        Ir.id = d.dname;
        kind =
          Ir.Structure
            { params; members = List.map (lower_member ~params ~diags) members };
        traits;
      }

(* Exposed for testing the primitive-keyword mapping in isolation, including its
   defensive default (the lexer only ever yields the keywords above). *)
module Internal = struct
  let prim_of_keyword = prim_of_keyword
end
