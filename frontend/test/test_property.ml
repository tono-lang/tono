open Tono_frontend
module G = QCheck.Gen

(* Applicative sugar so generators read like the records they build. *)
let ( let+ ) x f = G.map f x
let ( and+ ) a b = G.pair a b

(* Small fixed pools keep generated documents well-formed and readable while
   still exercising every variant. *)
let gen_ident = G.oneof_list [ "a#A"; "a#B"; "b#C"; "core#Page"; "core#List" ]
let gen_param = G.oneof_list [ "T"; "U"; "K"; "V" ]
let gen_name = G.oneof_list [ "id"; "amount"; "note"; "items"; "total"; "kind" ]
let gen_modname = G.oneof_list [ "payments"; "core"; "lab" ]

(* Finite floats whose canonical text is stable through a JSON round-trip. *)
let gen_float = G.oneof_list [ 0.; 1.; 2.5; 10.; 100.; -1.; 0.25 ]
let gen_opt g = G.oneof [ G.return None; G.map Option.some g ]

let gen_prim : Ir.prim G.t =
  G.oneof
    [
      G.oneof_list
        [
          Ir.Bool;
          Ir.String;
          Ir.Bytes;
          Ir.Float;
          Ir.Timestamp;
          Ir.Date;
          Ir.Duration;
          Ir.Uuid;
        ];
      (let+ bits = G.oneof_list [ 8; 16; 32; 64 ] and+ signed = G.bool in
       Ir.int_prim ~bits ~signed);
    ]

let gen_tref : Ir.tref G.t =
  G.sized_size (G.int_range 0 3)
    (G.fix (fun self n ->
         let leaf =
           G.oneof
             [
               G.map (fun p -> Ir.Prim p) gen_prim;
               G.map (fun s -> Ir.Param s) gen_param;
               G.map (fun id -> Ir.Ref (id, [])) gen_ident;
             ]
         in
         if n <= 0 then leaf
         else
           G.oneof
             [
               leaf;
               (let+ id = gen_ident
                and+ args = G.list_size (G.int_range 0 2) (self (n - 1)) in
                Ir.Ref (id, args));
               G.map (fun t -> Ir.List t) (self (n - 1));
               (let+ a = self (n - 1) and+ b = self (n - 1) in
                Ir.Map (a, b));
             ]))

(* Only core constraints; a Custom constraint never lives here. *)
let gen_constraint : Ir.constraint_ G.t =
  G.oneof
    [
      (let+ min = gen_opt gen_float
       and+ max = gen_opt gen_float
       and+ excl_min = G.bool
       and+ excl_max = G.bool in
       Ir.Range { min; max; excl_min; excl_max });
      (let+ min = gen_opt (G.int_range 0 1000)
       and+ max = gen_opt (G.int_range 0 1000) in
       Ir.Length { min; max });
      G.map
        (fun s -> Ir.Pattern s)
        (G.oneof_list [ "^[a-z]+$"; ".*"; "[0-9]{3}" ]);
      G.map (fun f -> Ir.MultipleOf f) (G.oneof_list [ 0.5; 1.; 2.; 0.25 ]);
    ]

(* Arbitrary JSON for defaults and trait values. Floats are deliberately left
   out so the property targets IR structure, not float text formatting. *)
let gen_json : Ir.json G.t =
  G.sized_size (G.int_range 0 2)
    (G.fix (fun self n ->
         let leaf =
           G.oneof
             [
               G.return `Null;
               G.map (fun b -> `Bool b) G.bool;
               G.map (fun i -> `Int i) (G.int_range (-1000) 1000);
               G.map
                 (fun s -> `String s)
                 (G.oneof_list [ "x"; "y"; ""; "hello" ]);
             ]
         in
         if n <= 0 then leaf
         else
           G.oneof
             [
               leaf;
               G.map
                 (fun xs -> `List xs)
                 (G.list_size (G.int_range 0 3) (self (n - 1)));
               G.map
                 (fun kvs -> `Assoc kvs)
                 (G.list_size (G.int_range 0 3)
                    (G.pair
                       (G.oneof_list [ "a"; "b"; "c"; "k" ])
                       (self (n - 1))));
             ]))

let gen_trait : Ir.trait G.t =
  let+ trait_id = gen_ident and+ value = gen_json in
  ({ trait_id; value } : Ir.trait)

let gen_member : Ir.member G.t =
  let+ name = gen_name
  and+ target = gen_tref
  and+ required = G.bool
  and+ default = gen_opt gen_json
  and+ constraints = G.list_size (G.int_range 0 2) gen_constraint
  and+ traits = G.list_size (G.int_range 0 2) gen_trait in
  ({ name; target; required; default; constraints; traits } : Ir.member)

let gen_members = G.list_size (G.int_range 0 3) gen_member
let gen_params = G.list_size (G.int_range 0 2) gen_param

let gen_enum_value =
  G.pair (G.oneof_list [ "a"; "b"; "c" ]) (gen_opt (G.int_range 0 100))

let gen_shape_kind : Ir.shape_kind G.t =
  G.oneof
    [
      (let+ params = gen_params and+ members = gen_members in
       Ir.Structure { params; members });
      (let+ params = gen_params
       and+ members = gen_members
       and+ discriminator = G.oneof_list [ "type"; "kind"; "@type" ] in
       Ir.Union { params; members; discriminator });
      (let+ backing = G.oneof_list [ `String; `Int ]
       and+ values = G.list_size (G.int_range 0 3) gen_enum_value in
       Ir.Enum { backing; values });
      (let+ operations = G.list_size (G.int_range 0 3) gen_ident in
       Ir.Service { operations });
      (let+ input = gen_opt gen_tref
       and+ output = gen_opt gen_tref
       and+ errors = G.list_size (G.int_range 0 2) gen_tref in
       Ir.Operation { input; output; errors });
    ]

let gen_shape : Ir.shape G.t =
  let+ id = gen_ident
  and+ kind = gen_shape_kind
  and+ traits = G.list_size (G.int_range 0 2) gen_trait in
  ({ id; kind; traits } : Ir.shape)

let gen_module : Ir.module_ G.t =
  let+ mod_name = gen_modname
  and+ shapes = G.list_size (G.int_range 0 3) gen_shape
  and+ operations = G.list_size (G.int_range 0 2) gen_shape in
  ({ mod_name; shapes; operations } : Ir.module_)

let gen_model : Ir.model G.t =
  let+ modules = G.list_size (G.int_range 0 2) gen_module in
  ({ tono_ir_version = Ir_json.current_ir_version; modules } : Ir.model)

let print_model m = Ir_json.to_canonical_string (Ir_json.encode_model m)

let roundtrip =
  QCheck.Test.make ~count:1000 ~name:"decode (parse (to_string (encode m))) = m"
    (QCheck.make ~print:print_model gen_model) (fun m ->
      let j = Ir_json.encode_model m in
      (* Go through the full text pipe (serialize then parse) so the property
         exercises string emission and parsing, not just the in-memory AST. *)
      let parsed = Yojson.Safe.from_string (Yojson.Safe.to_string j) in
      match Ir_json.decode_model parsed with
      | Error _ -> false
      | Ok m' ->
          String.equal
            (Ir_json.to_canonical_string j)
            (Ir_json.to_canonical_string (Ir_json.encode_model m')))

let () = QCheck_base_runner.run_tests_main [ roundtrip ]
