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

(* Map a surface type reference to its fully-qualified IR shape id. [qualifier] is
   the import qualifier for a cross-module reference ([common.Money]), or [None]
   for an in-module name. Lowering is purely syntactic: it names the id it is
   told to; existence and visibility are the resolve pass's concern. *)
type ref_resolver = qualifier:string option -> name:string -> Ir.shape_id

(* [module#name], the canonical namespaced id. An empty module name (the
   single-file default) leaves the name bare so a lone module stays unqualified. *)
let qualify (module_name : string) (name : string) : Ir.shape_id =
  if String.equal module_name "" then name else module_name ^ "#" ^ name

(* [params] are the type-parameter names in scope (a shape's generic header), so
   a bare name that matches one lowers to [Param] rather than a named [Ref]. *)
let rec lower_type ~(params : string list) ~(resolve : ref_resolver)
    ~(diags : Diagnostic.t list ref) (t : Ast.ty) : Ir.tref =
  match t with
  | Ast.TPrim (kw, _) -> Ir.Prim (prim_of_keyword kw)
  | Ast.TName (name, [], span) ->
      if String.equal name "decimal" then (
        report diags
          (Diagnostic.error span
             "there is no 'decimal' type; model money as an integer of minor \
              units, or use 'float'");
        Ir.Ref (resolve ~qualifier:None ~name, []))
      else if List.mem name params then Ir.Param name
      else Ir.Ref (resolve ~qualifier:None ~name, [])
  | Ast.TName (name, args, _) ->
      Ir.Ref
        ( resolve ~qualifier:None ~name,
          List.map (lower_type ~params ~resolve ~diags) args )
  | Ast.TQName (qualifier, name, args, _) ->
      Ir.Ref
        ( resolve ~qualifier:(Some qualifier) ~name,
          List.map (lower_type ~params ~resolve ~diags) args )
  | Ast.TList (inner, _) -> Ir.List (lower_type ~params ~resolve ~diags inner)
  | Ast.TMap (k, v, _) ->
      Ir.Map
        ( lower_type ~params ~resolve ~diags k,
          lower_type ~params ~resolve ~diags v )
  | Ast.TNullable (inner, _) ->
      (* Nullability is a member-level flag; at the type level the inner type is
         what reaches the IR. The member lowering reads the [?] separately. *)
      lower_type ~params ~resolve ~diags inner
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
  | Ast.ARef r ->
      (* A field reference in a trait value keeps its path structured, so the
         Protocol resolver (and the backend) never re-parse ".a.b" strings. *)
      `Assoc [ ("field", `List (List.map (fun s -> `String s) r.Ast.segs)) ]
  | Ast.AKv (k, v) -> `Assoc [ (k, json_of_arg v) ]
  | Ast.AList xs -> `List (List.map json_of_arg xs)
  | Ast.ACtor c ->
      (* Kept structured (not folded through the generic AKv/field encoding)
         so the Protocol resolver can tell a ctor mapper apart from an
         ordinary keyword argument without re-parsing anything. *)
      `Assoc
        [
          ("ctor", `String c.Ast.ctor_name);
          ( "fields",
            `Assoc
              (List.map (fun (n, _, v) -> (n, json_of_arg v)) c.Ast.ctor_fields)
          );
        ]
  | Ast.ACall ce ->
      (* An extern call inside a trait argument, e.g.
         @header("Authorization", companyauth.sign(.request)). Resolving [ns]
         against a declared [ext] block is out of scope here; this only keeps
         the call structured for later resolution. *)
      `Assoc
        [
          ( "call",
            `Assoc
              [ ("ns", `String ce.Ast.ce_ns); ("fn", `String ce.Ast.ce_fn) ] );
          ("args", `List (List.map json_of_call_arg ce.Ast.ce_args));
        ]

and json_of_call_arg : Ast.call_arg -> Ir.json = function
  | Ast.CaParam (n, _) -> `Assoc [ ("param", `String n) ]
  | Ast.CaRef r ->
      `Assoc [ ("field", `List (List.map (fun s -> `String s) r.Ast.segs)) ]
  | Ast.CaCtor c -> json_of_arg (Ast.ACtor c)
  | Ast.CaLit (Ast.LStr s, _) -> `Assoc [ ("lit", `String s) ]
  | Ast.CaLit (Ast.LInt n, _) -> `Assoc [ ("lit", `Int n) ]
  | Ast.CaLit (Ast.LFloat f, _) -> `Assoc [ ("lit", `Float f) ]
  | Ast.CaCall nc ->
      (* A nested foreign-symbol call, e.g. [WithPrecision(precision)]
         inside a language block's own [call:] line -- [Protocol_http] never
         reaches this shape (a trait argument's own call has no [call:] line
         to nest inside; [check_request_value] already rejects a bare
         identifier there), so it is only ever read back through
         [Lower_extern.lower_call_arg]'s IR path, never through
         [Protocol_http]'s [call_arg_value_of]. *)
      `Assoc
        [
          ("symbol", `String nc.Ast.nc_symbol);
          ("symbol_args", `List (List.map json_of_call_arg nc.Ast.nc_args));
        ]
  | Ast.CaList (items, _) ->
      `Assoc [ ("list", `List (List.map json_of_call_arg items)) ]
  | Ast.CaParamAs (n, _, sp, _) ->
      `Assoc [ ("param", `String n); ("as", `String sp) ]
  | Ast.CaForeign (s, _) -> `Assoc [ ("foreign", `String s) ]

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

(* The trait name is emitted bare; namespace resolution is a later pass. *)
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

(* ── Entry fields ──────────────────────────────────────────────────────── *)

(* The transform a catalog trait names, e.g. "str::trim" -> "trim". Catalog
   membership is a typecheck concern; lowering keeps any "str::" suffix. *)
let transform_of (tname : string) : string option =
  let prefix = "str::" in
  let plen = String.length prefix in
  if String.length tname > plen && String.equal (String.sub tname 0 plen) prefix
  then Some (String.sub tname plen (String.length tname - plen))
  else None

let lower_source ~diags (tr : Ast.trait) : Ir.source option =
  let marker source =
    if tr.Ast.targs <> [] then
      report diags
        (Diagnostic.error tr.tspan
           (Printf.sprintf "@%s takes no arguments" tr.tname));
    Some source
  in
  match tr.Ast.tname with
  | "arg" -> marker Ir.Arg
  | "with" -> marker Ir.With
  | "env" -> (
      match tr.targs with
      | [ Ast.AString s ] -> Some (Ir.Env (Ir.Env_name s))
      | [ Ast.ARef r ] -> Some (Ir.Env (Ir.Env_field r.segs))
      | _ ->
          report diags
            (Diagnostic.error tr.tspan
               "@env expects a single variable name string or a field reference");
          None)
  | "default" ->
      (match tr.targs with
      | [] | [ _ ] -> ()
      | _ ->
          report diags
            (Diagnostic.error tr.tspan "@default takes a single value"));
      let v = match tr.targs with a :: _ -> json_of_arg a | [] -> `Null in
      Some (Ir.Default v)
  | _ -> None

let lower_pattern : Ast.match_pattern -> Ir.json option = function
  | Ast.PString s -> Some (`String s)
  | Ast.PInt n -> Some (`Int n)
  | Ast.PName "true" -> Some (`Bool true)
  | Ast.PName "false" -> Some (`Bool false)
  | Ast.PName n -> Some (`String n) (* an enum case name *)
  | Ast.PWildcard -> None
  (* Not a bare JSON `null`: Rust's serde treats a present `null` on an
     `Option<Value>` field the same as an absent field (both decode to
     [None]), which would collapse this back into the wildcard on that
     side. A marker object survives the round-trip on both sides. *)
  | Ast.PNull -> Some (`Assoc [ ("null", `Bool true) ])

let lower_arm_value ~diags : Ast.arm_value -> Ir.arm_value = function
  | Ast.AVRef r -> Ir.Arm_field r.segs
  | Ast.AVString s -> Ir.Arm_lit (`String s)
  | Ast.AVInt n -> Ir.Arm_lit (`Int n)
  | Ast.AVName "true" -> Ir.Arm_lit (`Bool true)
  | Ast.AVName "false" -> Ir.Arm_lit (`Bool false)
  | Ast.AVName n -> Ir.Arm_lit (`String n)
  | Ast.AVSubject _ -> Ir.Arm_subject
  | Ast.AVSources traits ->
      Ir.Arm_sources
        (List.filter_map
           (fun (tr : Ast.trait) ->
             match tr.tname with
             | "arg" | "with" | "env" | "default" -> lower_source ~diags tr
             | _ ->
                 report diags
                   (Diagnostic.error tr.tspan
                      "a match arm only stacks value sources (@env/@default)");
                 None)
           traits)

let lower_select ~diags (fm : Ast.field_match) : Ir.select =
  {
    Ir.subject = fm.subject.segs;
    subject_index =
      Option.map (fun (idx : Ast.ref_path) -> idx.Ast.segs) fm.subject.Ast.index;
    arms =
      List.map
        (fun (a : Ast.match_arm) ->
          {
            Ir.arm_pattern = lower_pattern a.pat;
            arm_value = lower_arm_value ~diags a.value;
          })
        fm.arms;
  }

(* A handle method call, shared by an op's own "impl" body and a field's own
   value source: the receiver path, the method, and the arguments. *)
let lower_handle_call (hc : Ast.op_impl) : Ir.op_impl_call =
  {
    Ir.oic_recv = hc.Ast.oi_recv.Ast.segs;
    oic_method = hc.Ast.oi_method;
    oic_args = List.map Lower_extern.lower_call_arg hc.Ast.oi_args;
  }

(* Extern call lowering lives in [Lower_extern] (shared with the ext/extern
   library block); aliased locally. *)
let lower_call_expr = Lower_extern.lower_call_expr

let lower_entry_field ~resolve ~diags (m : Ast.member) : Ir.entry_field =
  check_snake diags m.mname_span "member name" m.mname;
  (* Nullability on an entry/config field is a typecheck error (optionality
     comes from the sources); lowering keeps the base type either way. *)
  let base = match m.mtype with Ast.TNullable (t, _) -> t | t -> t in
  let target = lower_type ~params:[] ~resolve ~diags base in
  let sources = ref [] in
  let transforms = ref [] in
  let binds = ref [] in
  let format = ref None in
  let constraints = ref [] in
  let bag = ref [] in
  List.iter
    (fun (tr : Ast.trait) ->
      match tr.tname with
      | "arg" | "with" | "env" | "default" -> (
          match lower_source ~diags tr with
          | Some s -> sources := !sources @ [ s ]
          | None -> ())
      | "format" -> (
          match tr.targs with
          | [ Ast.AString s ] ->
              format := Some (Template.parse ~diags ~span:tr.tspan s)
          | _ ->
              report diags
                (Diagnostic.error tr.tspan
                   "@format expects a single template string"))
      | "bind" -> (
          match tr.targs with
          | [ Ast.AName tgt; Ast.ARef r ] ->
              binds :=
                !binds @ [ { Ir.bind_field = tgt; bind_source = r.segs } ]
          | _ ->
              report diags
                (Diagnostic.error tr.tspan
                   "@bind expects a target field name and a source reference, \
                    e.g. @bind(api_key, .api_key)"))
      | "range" | "length" | "pattern" | "multipleOf" -> (
          match constraint_of_trait diags tr with
          | Some c -> constraints := !constraints @ [ c ]
          | None -> ())
      | name -> (
          match transform_of name with
          | Some t -> transforms := !transforms @ [ t ]
          | None -> bag := !bag @ [ bag_trait tr ]))
    m.mtraits;
  {
    Ir.ef_name = m.mname;
    ef_target = target;
    ef_sources = !sources;
    ef_format = !format;
    ef_transforms = !transforms;
    ef_select =
      (match m.mvalue with
      | Some (Ast.MMatch fm) -> Some (lower_select ~diags fm)
      | Some (Ast.MCall _ | Ast.MHandleCall _) | None -> None);
    ef_call =
      (match m.mvalue with
      | Some (Ast.MCall ce) -> Some (lower_call_expr ce)
      | Some (Ast.MMatch _ | Ast.MHandleCall _) | None -> None);
    ef_handle_call =
      (match m.mvalue with
      | Some (Ast.MHandleCall hc) -> Some (lower_handle_call hc)
      | Some (Ast.MMatch _ | Ast.MCall _) | None -> None);
    ef_binds = !binds;
    ef_constraints = !constraints;
    ef_traits = !bag;
  }

(* ── Members ───────────────────────────────────────────────────────────── *)

let lower_member ~params ~resolve ~diags (m : Ast.member) : Ir.member =
  check_snake diags m.mname_span "member name" m.mname;
  let base, nullable =
    match m.mtype with Ast.TNullable (t, _) -> (t, true) | t -> (t, false)
  in
  let target = lower_type ~params ~resolve ~diags base in
  let required = ref (not nullable) in
  let default = ref None in
  let constraints = ref [] in
  let bag = ref [] in
  List.iter
    (fun (tr : Ast.trait) ->
      match tr.tname with
      | "required" ->
          (* The nullable-vs-required conflict is a semantic check the
             typechecker reports (TC0007); lowering only records the state. *)
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

(* Lower an operation's traits and body into an Operation shape under [id].
   Shared by top-level ops and ops nested in an entry body. *)
let lower_op_shape ~id ~resolve ~diags (d : Ast.decl) ~pname ~input ~output
    ~oimpl ~pub_trait : Ir.shape =
  let lower_opt = Option.map (lower_type ~params:[] ~resolve ~diags) in
  (* A [T?] return is carried as a flag next to [output], the same shape as a
     member's [required]: nullability is not a type node in the IR. *)
  let output, output_nullable =
    match output with
    | Some (Ast.TNullable (t, _)) -> (Some t, true)
    | o -> (o, false)
  in
  (* @errors(A, B) lists the operation's error types by name; repeated
     @errors traits accumulate. A non-name argument has no type to point
     at, so it is diagnosed rather than silently dropped. Only same-module
     names resolve here (qualifier:None): a trait argument is a bare identifier
     in the grammar, so a qualified [common.NotFound] does not parse as one
     token; cross-module error references need a qualified trait-argument form,
     left to a focused follow-up. *)
  let errs, rest = take_trait "errors" d.dtraits in
  let errors =
    List.concat_map
      (fun (tr : Ast.trait) ->
        List.filter_map
          (function
            | Ast.AName n -> Some (Ir.Ref (resolve ~qualifier:None ~name:n, []))
            | _ ->
                report diags
                  (Diagnostic.error tr.tspan "@errors expects type names");
                None)
          tr.Ast.targs)
      errs
  in
  {
    Ir.id;
    kind =
      Ir.Operation
        {
          input = lower_opt input;
          input_name = pname;
          output = lower_opt output;
          output_nullable;
          errors;
          wire = None;
          (* Protocol resolution happens strictly after lowering. *)
          impl_call = Option.map lower_handle_call oimpl;
        };
    traits = pub_trait @ lower_bag_traits rest;
  }

let lower_decl ?(role = Roles.Wire) ~module_name ~resolve ~diags (d : Ast.decl)
    : Ir.shape =
  check_snake diags d.dname_span "shape name" d.dname;
  let pub_trait =
    if d.pub then [ { Ir.trait_id = "pub"; value = `Null } ] else []
  in
  let bag rest = pub_trait @ lower_bag_traits rest in
  match d.dkind with
  | Ast.DStruct { params = _; members; ops; slangs = _ } when role = Roles.Entry
    ->
      (* Entry generics have no meaning (the construction surface is concrete);
         the typechecker rejects them, lowering drops them. Nested ops are
         identified as [module#entry.op], scoped to the entry so they never
         collide with a top-level shape. *)
      let fields = List.map (lower_entry_field ~resolve ~diags) members in
      let operations =
        List.map
          (fun (op : Ast.decl) ->
            check_snake diags op.dname_span "operation name" op.dname;
            match op.dkind with
            | Ast.DOp { pname; input; output; oimpl } ->
                lower_op_shape
                  ~id:(qualify module_name (d.dname ^ "." ^ op.dname))
                  ~resolve ~diags op ~pname ~input ~output ~oimpl ~pub_trait:[]
            | _ -> assert false)
          ops
      in
      {
        Ir.id = qualify module_name d.dname;
        kind = Ir.Entry { fields; operations };
        traits = bag d.dtraits;
      }
  | Ast.DStruct { params = _; members; ops = _; slangs = _ }
    when role = Roles.Config ->
      {
        Ir.id = qualify module_name d.dname;
        kind =
          Ir.Config
            { fields = List.map (lower_entry_field ~resolve ~diags) members };
        traits = bag d.dtraits;
      }
  | Ast.DStruct { params; members; ops = _; slangs } ->
      {
        Ir.id = qualify module_name d.dname;
        kind =
          Ir.Structure
            {
              params;
              members = List.map (lower_member ~params ~resolve ~diags) members;
            };
        traits = bag d.dtraits @ Lower_extern.foreign_trait slangs;
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
          | Some t -> lower_type ~params ~resolve ~diags t
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
        Ir.id = qualify module_name d.dname;
        kind =
          Ir.union ?discriminator ~params
            ~members:(List.map variant variants)
            ();
        traits = bag rest;
      }
  | Ast.DEnum { cases } ->
      let int_backed =
        List.exists (fun (c : Ast.enum_case) -> c.cint <> None) cases
      in
      let backing = if int_backed then `Int else `String in
      let values =
        List.map
          (fun (c : Ast.enum_case) ->
            check_snake diags c.cname_span "enum case" c.cname;
            (* Backing consistency (an int-backed case missing its value) is a
               semantic check the typechecker reports (TC0009). Case traits ride
               the bag, exactly like shape and member traits; @doc lives there. *)
            Ir.enum_value ?int:c.cint
              ~traits:(lower_bag_traits c.ctraits)
              c.cname)
          cases
      in
      {
        Ir.id = qualify module_name d.dname;
        kind = Ir.Enum { backing; values };
        traits = bag d.dtraits;
      }
  | Ast.DOp { pname; input; output; oimpl } ->
      lower_op_shape
        ~id:(qualify module_name d.dname)
        ~resolve ~diags d ~pname ~input ~output ~oimpl ~pub_trait
  | Ast.DExt _ | Ast.DExtLib _ | Ast.DTest _ ->
      (* Extensions, ext libs, and tests have no shape; [lower_file] routes
         extensions to [lower_ext], ext libs to [lower_ext_lib], and leaves
         tests to the typechecker, which lowers them with full type
         information. *)
      assert false

let default_resolver ~module_name : ref_resolver =
 fun ~qualifier ~name ->
  match qualifier with
  | None -> qualify module_name name
  | Some q -> qualify q name

(* ── Extensions ────────────────────────────────────────────────────────── *)

(* Signature refs are namespaced through [resolve] but their existence is not
   checked (a hook's input/output can be a runtime type like [canonical_request]);
   binding-vs-signature validation is deferred. *)
let lower_ext_sig ~resolve ~diags (s : Ast.ext_sig) : Ir.ext_sig =
  {
    Ir.input = lower_type ~params:[] ~resolve ~diags s.esig_in;
    output = lower_type ~params:[] ~resolve ~diags s.esig_out;
  }

let lower_ext ~resolve ~diags (d : Ast.decl) : Ir.extension =
  (* An impl name may be the dotted "entry.op" form, so each segment is checked
     on its own; every other kind is a single segment and the split is a no-op. *)
  List.iter
    (fun seg -> check_snake diags d.dname_span "extension name" seg)
    (String.split_on_char '.' d.dname);
  match d.dkind with
  | Ast.DExt { ekind; esig; eraw; ebindings; econformance; _ } ->
      let ext_kind =
        match ekind with
        | Ast.EHook ->
            (* [lower_file] filters hook decls out before calling [lower_ext]. *)
            assert false
        | Ast.EContract -> Ir.Contract
        | Ast.EConstraint -> Ir.Constraint
        | Ast.EImpl -> Ir.Impl
      in
      let ext_bindings =
        List.map (fun (b : Ast.ext_binding) -> (b.lang, b.target)) ebindings
      in
      {
        Ir.ext_name = d.dname;
        ext_kind;
        ext_sig = Option.map (lower_ext_sig ~resolve ~diags) esig;
        ext_raw = Option.is_some eraw;
        ext_bindings;
        ext_conformance = econformance;
      }
  | _ -> assert false

(* The [ext <name> { ... }] library block lowers in [Lower_extern] (which
   also owns the shared extern-call lowering above); [lower_type]/
   [lower_select] are threaded in to avoid a dependency cycle. *)
let lower_ext_lib = Lower_extern.lower_ext_lib ~lower_type ~lower_select

(* Lower a whole file into a module: operations land in [operations], extensions
   in [extensions], every other shape in [shapes], preserving declaration order.
   Shape ids and references are namespaced through [resolve]; the default (used
   when a file is compiled on its own) qualifies own names with [module_name] and
   treats a qualifier as a module directly, which the resolve pass then flags as
   an unknown import. Imports carry no runtime IR: they only steer resolution. *)
let lower_file ~module_name ?resolve ~diags (file : Ast.file) : Ir.module_ =
  let resolve =
    match resolve with Some r -> r | None -> default_resolver ~module_name
  in
  let roles = Roles.classify file.Ast.decls in
  let shapes_rev = ref [] in
  let ops_rev = ref [] in
  let exts_rev = ref [] in
  let ext_libs_rev = ref [] in
  List.iter
    (fun (d : Ast.decl) ->
      match d.dkind with
      (* A hook is always rejected by [Check_ext] (it carries no valid
         lifecycle anymore), so it lowers to no IR extension at all; the IR's
         [ext_kind] never needs to represent one. *)
      | Ast.DExt { ekind = Ast.EHook; _ } -> ()
      | Ast.DExt _ -> exts_rev := lower_ext ~resolve ~diags d :: !exts_rev
      | Ast.DExtLib _ ->
          ext_libs_rev := lower_ext_lib ~resolve ~diags d :: !ext_libs_rev
      (* Tests lower during typecheck (their wire encoding is type-driven);
         they carry no shape and their names are free strings. *)
      | Ast.DTest _ -> ()
      | _ -> (
          let role = Roles.role_of roles d.dname in
          let shape = lower_decl ~role ~module_name ~resolve ~diags d in
          match shape.Ir.kind with
          | Ir.Operation _ -> ops_rev := shape :: !ops_rev
          | _ -> shapes_rev := shape :: !shapes_rev))
    file.Ast.decls;
  {
    Ir.mod_name = module_name;
    shapes = List.rev !shapes_rev;
    operations = List.rev !ops_rev;
    extensions = List.rev !exts_rev;
    ext_libs = List.rev !ext_libs_rev;
    tests = [];
  }

(* Exposed for testing the primitive-keyword mapping in isolation, including its
   defensive default (the lexer only ever yields the keywords above). *)
module Internal = struct
  let prim_of_keyword = prim_of_keyword
end
