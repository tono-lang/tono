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
  (* An empty name only arises from an already-diagnosed parse error, so skip it
     rather than pile on a misleading "must be snake_case" report. *)
  if (not (String.equal name "")) && not (is_snake_case name) then
    report diags
      (Diagnostic.error span
         (Printf.sprintf "%s '%s' must be snake_case" what name))

(* ── Trait argument JSON ───────────────────────────────────────────────── *)

let rec json_of_arg : Ast.trait_arg -> Ir.json = function
  | Ast.AString s -> `String s
  | Ast.AInt n -> `Int n
  | Ast.AFloat f -> `Float f
  | Ast.AName s -> `String s
  | Ast.AKv (k, v) -> `Assoc [ (k, json_of_arg v) ]

(* All-keyword args collapse to a single object (@http(method: "get", path: "/x")
   -> {"method":"get","path":"/x"}); any positional arg keeps the uniform array
   form, where each keyword arg stays a single-key object. *)
let json_of_args : Ast.trait_arg list -> Ir.json = function
  | [] -> `Null
  | args ->
      let to_pair = function
        | Ast.AKv (k, v) -> Some (k, json_of_arg v)
        | _ -> None
      in
      let pairs = List.filter_map to_pair args in
      if List.length pairs = List.length args then `Assoc pairs
      else `List (List.map json_of_arg args)

(* The bare trait name; the module-resolution pass qualifies it (core# or
   module#) once the full set of declared names is known. *)
let bag_trait (tr : Ast.trait) : Ir.trait =
  { Ir.trait_id = tr.tname; value = json_of_args tr.targs }

let lower_bag_traits trs = List.map bag_trait trs

(* ── Core constraint lifting ───────────────────────────────────────────── *)

let kv_arg key args =
  List.find_map
    (function Ast.AKv (k, v) when String.equal k key -> Some v | _ -> None)
    args

let int_of_arg = function Ast.AInt n -> Some n | _ -> None

let float_of_arg = function
  | Ast.AInt n -> Some (float_of_int n)
  | Ast.AFloat f -> Some f
  | _ -> None

(* Bounds accept either positional [min, max] (numbers) or keyword [min:, max:]
   forms. Two positional keyword args must not match here -- they belong to the
   keyword branch. *)
let range_bounds args =
  match args with
  | [ ((Ast.AInt _ | Ast.AFloat _) as a); ((Ast.AInt _ | Ast.AFloat _) as b) ]
    ->
      (float_of_arg a, float_of_arg b)
  | _ ->
      ( Option.bind (kv_arg "min" args) float_of_arg,
        Option.bind (kv_arg "max" args) float_of_arg )

let length_bounds args =
  match args with
  | [ Ast.AInt a; Ast.AInt b ] -> (Some a, Some b)
  | _ ->
      ( Option.bind (kv_arg "min" args) int_of_arg,
        Option.bind (kv_arg "max" args) int_of_arg )

(* Bounds given but none recognized (e.g. [@range(5)] or [@range("x")]) is a
   silent no-op otherwise, so flag it rather than emit an empty constraint. *)
let warn_unparsed_bounds diags (tr : Ast.trait) min max =
  if tr.targs <> [] && min = None && max = None then
    report diags
      (Diagnostic.error tr.tspan
         (Printf.sprintf
            "@%s expects (min, max) or (min: N, max: N) numeric bounds" tr.tname))

(* A [min:]/[max:] keyword arg whose value is not numeric is dropped silently
   otherwise (e.g. [@range(min: 5, max: "x")] keeps min, loses max), so flag each
   such argument individually. [numeric] is [float_of_arg] or [int_of_arg]. *)
let warn_bad_bound_kvs diags (tr : Ast.trait) numeric =
  List.iter
    (function
      | Ast.AKv ((("min" | "max") as k), v) when numeric v = None ->
          report diags
            (Diagnostic.error tr.tspan
               (Printf.sprintf "@%s %s must be a number" tr.tname k))
      | _ -> ())
    tr.targs

let constraint_of_trait diags (tr : Ast.trait) : Ir.constraint_ option =
  match tr.tname with
  | "range" ->
      let min, max = range_bounds tr.targs in
      warn_unparsed_bounds diags tr min max;
      warn_bad_bound_kvs diags tr float_of_arg;
      Some (Ir.range ?min ?max ())
  | "length" ->
      let min, max = length_bounds tr.targs in
      warn_unparsed_bounds diags tr min max;
      warn_bad_bound_kvs diags tr int_of_arg;
      Some (Ir.length ?min ?max ())
  | "pattern" -> (
      match tr.targs with
      | Ast.AString s :: _ -> Some (Ir.pattern s)
      | _ ->
          report diags
            (Diagnostic.error tr.tspan "@pattern expects a string argument");
          None)
  | "multipleOf" -> (
      match Option.bind (List.nth_opt tr.targs 0) float_of_arg with
      | Some f -> Some (Ir.multiple_of f)
      | None ->
          report diags
            (Diagnostic.error tr.tspan "@multipleOf expects a number argument");
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
      | "required" ->
          if nullable then
            report diags
              (Diagnostic.error m.mname_span
                 (Printf.sprintf
                    "@required on the nullable member '%s' is contradictory; \
                     drop the '?' or the @required"
                    m.mname));
          required := true
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

(* Pull a named shape-level trait out of the bag, returning the matches and the
   rest. Used for [@open]/[@discriminator], which become structured IR fields
   rather than bag entries. *)
let take_trait name (traits : Ast.trait list) =
  List.partition (fun (t : Ast.trait) -> String.equal t.tname name) traits

let lower_decl ~diags (d : Ast.decl) : Ir.shape =
  check_snake diags d.dname_span "shape name" d.dname;
  let pub_trait =
    if d.pub then [ { Ir.trait_id = "pub"; value = `Null } ] else []
  in
  let bag rest = pub_trait @ lower_bag_traits rest in
  match d.dkind with
  | Ast.DStruct { params; members } ->
      {
        Ir.id = d.dname;
        kind =
          Ir.Structure
            { params; members = List.map (lower_member ~params ~diags) members };
        traits = bag d.dtraits;
      }
  | Ast.DUnion { params; variants } ->
      let disc, rest = take_trait "discriminator" d.dtraits in
      let discriminator =
        match disc with
        | { Ast.targs = Ast.AString s :: _; _ } :: _ -> Some s
        | { Ast.tspan; _ } :: _ ->
            report diags
              (Diagnostic.error tspan "@discriminator expects a string argument");
            None
        | [] -> None
      in
      let variant (v : Ast.union_variant) : Ir.member =
        check_snake diags v.vname_span "variant" v.vname;
        let target =
          match v.vpayload with
          | Some t -> lower_type ~params ~diags t
          | None ->
              report diags
                (Diagnostic.error v.vname_span
                   (Printf.sprintf
                      "union variant '%s' needs a payload type, e.g. %s(T)"
                      v.vname v.vname));
              Ir.Ref ("", [])
        in
        {
          Ir.name = v.vname;
          target;
          required = true;
          default = None;
          constraints = [];
          traits = lower_bag_traits v.vtraits;
        }
      in
      {
        Ir.id = d.dname;
        kind =
          Ir.union ?discriminator ~params
            ~members:(List.map variant variants)
            ();
        traits = bag rest;
      }
  | Ast.DEnum { cases } ->
      let open_traits, rest = take_trait "open" d.dtraits in
      let open_ = open_traits <> [] in
      let int_backed =
        List.exists (fun (c : Ast.enum_case) -> c.cint <> None) cases
      in
      let backing = if int_backed then `Int else `String in
      let values =
        List.map
          (fun (c : Ast.enum_case) ->
            check_snake diags c.cname_span "enum case" c.cname;
            if c.ctraits <> [] then
              report diags
                (Diagnostic.error c.cname_span
                   "enum case traits are not supported");
            if int_backed && c.cint = None then
              report diags
                (Diagnostic.error c.cname_span
                   "every case of an int-backed enum needs an explicit '= \
                    value'");
            (c.cname, c.cint))
          cases
      in
      {
        Ir.id = d.dname;
        kind = Ir.Enum { backing; values; open_ };
        traits = bag rest;
      }
  | Ast.DOp { input; output } ->
      let lower_opt = Option.map (lower_type ~params:[] ~diags) in
      (* @errors(A, B) is lifted into Operation.errors; each arg is a type name. *)
      let errs, rest = take_trait "errors" d.dtraits in
      let errors =
        match errs with
        | tr :: _ ->
            List.filter_map
              (function Ast.AName n -> Some (Ir.Ref (n, [])) | _ -> None)
              tr.Ast.targs
        | [] -> []
      in
      {
        Ir.id = d.dname;
        kind =
          Ir.Operation
            { input = lower_opt input; output = lower_opt output; errors };
        traits = bag rest;
      }

(* Lower a whole file into a module: operations land in [operations], every other
   shape in [shapes], preserving declaration order. Names are emitted bare; the
   module/core namespacing is a later name-resolution pass (separate PRD). *)
let lower_file ~module_name ~diags (file : Ast.file) : Ir.module_ =
  let shapes_rev = ref [] in
  let ops_rev = ref [] in
  List.iter
    (fun d ->
      let shape = lower_decl ~diags d in
      match shape.Ir.kind with
      | Ir.Operation _ -> ops_rev := shape :: !ops_rev
      | _ -> shapes_rev := shape :: !shapes_rev)
    file;
  {
    Ir.mod_name = module_name;
    shapes = List.rev !shapes_rev;
    operations = List.rev !ops_rev;
  }

(* Exposed for testing the primitive-keyword mapping in isolation, including its
   defensive default (the lexer only ever yields the keywords above). *)
module Internal = struct
  let prim_of_keyword = prim_of_keyword
end
