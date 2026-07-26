(* The calculus is self-referential: the reference evaluator is the truth, and
   the committed vectors in calculus_vectors.json pin its input -> output
   relation. A target that lowers the calculus must reproduce these outputs bit
   for bit against the same file, so the vectors verify lowering fidelity, not
   logic. The gate stays deliberately light (no mutation): a well-typed program
   already compiles into equivalent total code by construction.

   Value encoding in the file: one-key objects tag the type. i8/i16/i32 and
   u8/u16/u32 are JSON numbers; i64/u64 are decimal strings (the wire rule);
   the rest are {"f64":x}, {"str":s}, {"bool":b}, {"list":[..]},
   {"map":[[k,v],..]}, {"struct":{field:v}}, {"variant":[name,v]}, and
   {"opt":v|null}. *)
open Tono_frontend
module J = Yojson.Safe.Util

let rec value_of_json (j : Yojson.Safe.t) : Calc_eval.value =
  let open Calc_eval in
  match j with
  | `Assoc [ ("i8", `Int n) ] -> VInt (Int64.of_int n, 8, true)
  | `Assoc [ ("i16", `Int n) ] -> VInt (Int64.of_int n, 16, true)
  | `Assoc [ ("i32", `Int n) ] -> VInt (Int64.of_int n, 32, true)
  | `Assoc [ ("u8", `Int n) ] -> VInt (Int64.of_int n, 8, false)
  | `Assoc [ ("u16", `Int n) ] -> VInt (Int64.of_int n, 16, false)
  | `Assoc [ ("u32", `Int n) ] -> VInt (Int64.of_int n, 32, false)
  | `Assoc [ ("i64", `String s) ] -> VInt (Int64.of_string s, 64, true)
  | `Assoc [ ("u64", `String s) ] -> VInt (Int64.of_string s, 64, false)
  | `Assoc [ ("f64", `Float f) ] -> VFloat f
  | `Assoc [ ("f64", `Int n) ] -> VFloat (float_of_int n)
  | `Assoc [ ("str", `String s) ] -> VStr s
  | `Assoc [ ("bool", `Bool b) ] -> VBool b
  | `Assoc [ ("list", `List xs) ] -> VList (List.map value_of_json xs)
  | `Assoc [ ("map", `List pairs) ] ->
      VMap
        (List.map
           (function
             | `List [ k; v ] -> (value_of_json k, value_of_json v)
             | _ -> failwith "map entry must be a [key, value] pair")
           pairs)
  | `Assoc [ ("struct", `Assoc fields) ] ->
      VStruct (List.map (fun (f, v) -> (f, value_of_json v)) fields)
  | `Assoc [ ("variant", `List [ `String name; v ]) ] ->
      VVariant (name, value_of_json v)
  | `Assoc [ ("opt", `Null) ] -> VOpt None
  | `Assoc [ ("opt", v) ] -> VOpt (Some (value_of_json v))
  | _ -> failwith ("unknown value encoding: " ^ Yojson.Safe.to_string j)

let rec show (v : Calc_eval.value) =
  let open Calc_eval in
  match v with
  | VInt (n, bits, signed) ->
      Printf.sprintf "%s:%c%d" (Int64.to_string n)
        (if signed then 'i' else 'u')
        bits
  | VFloat f -> Printf.sprintf "%h" f
  | VStr s -> Printf.sprintf "%S" s
  | VBool b -> string_of_bool b
  | VList xs -> "[" ^ String.concat "; " (List.map show xs) ^ "]"
  | VMap pairs ->
      "{"
      ^ String.concat "; "
          (List.map (fun (k, v) -> show k ^ " -> " ^ show v) pairs)
      ^ "}"
  | VStruct fields ->
      "{"
      ^ String.concat "; " (List.map (fun (f, v) -> f ^ ": " ^ show v) fields)
      ^ "}"
  | VVariant (name, v) -> name ^ "(" ^ show v ^ ")"
  | VOpt None -> "None"
  | VOpt (Some v) -> "Some(" ^ show v ^ ")"

let value_t =
  Alcotest.testable (fun ppf v -> Format.pp_print_string ppf (show v)) ( = )

let run_vector (v : Yojson.Safe.t) =
  let name = J.member "name" v |> J.to_string in
  let source = J.member "program" v |> J.to_string in
  let entry = J.member "entry" v |> J.to_string in
  let args = J.member "args" v |> J.to_list |> List.map value_of_json in
  let expected = value_of_json (J.member "expected" v) in
  let program, diags = Calc_parser.parse source in
  Alcotest.(check int) (name ^ ": parses clean") 0 (List.length diags);
  Alcotest.check value_t name expected (Calc_eval.eval_fn program entry args)

let vectors () =
  let file = Yojson.Safe.from_file "calculus_vectors.json" in
  let vs = J.member "vectors" file |> J.to_list in
  Alcotest.(check bool) "vector suite is not empty" true (vs <> []);
  List.iter run_vector vs

let () =
  Alcotest.run "calculus_vectors"
    [
      ( "vectors",
        [ Alcotest.test_case "reference evaluator vectors" `Quick vectors ] );
    ]
