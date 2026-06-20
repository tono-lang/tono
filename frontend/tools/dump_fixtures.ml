(* Emits and verifies the golden JSON fixtures for the canonical IR corpus.

   write <dir>  -- (re)generate <dir>/<name>.json for every corpus example
   check <dir>  -- assert each <dir>/<name>.json still matches the encoder,
                   exiting non-zero on any mismatch or missing file *)

open Tono_frontend

let encode model = Ir_json.to_canonical_string (Ir_json.encode_model model)

let read_file path =
  let ic = open_in_bin path in
  Fun.protect
    ~finally:(fun () -> close_in ic)
    (fun () -> really_input_string ic (in_channel_length ic))

let write_one dir (name, model) =
  let path = Filename.concat dir (name ^ ".json") in
  let oc = open_out_bin path in
  Fun.protect
    ~finally:(fun () -> close_out oc)
    (fun () ->
      output_string oc (encode model);
      output_char oc '\n');
  Printf.printf "wrote %s\n" path

let check_one dir (name, model) =
  let path = Filename.concat dir (name ^ ".json") in
  let expected = encode model in
  if not (Sys.file_exists path) then (
    Printf.eprintf "missing fixture: %s\n" path;
    false)
  else
    (* Compare canonically so trailing newline / key order never matter. *)
    let actual =
      Ir_json.to_canonical_string (Yojson.Safe.from_string (read_file path))
    in
    if String.equal actual expected then true
    else (
      Printf.eprintf "fixture mismatch: %s\n  expected: %s\n  actual:   %s\n"
        name expected actual;
      false)

let emit_one name =
  match List.assoc_opt name Ir_corpus.examples with
  | Some model -> print_string (encode model)
  | None ->
      Printf.eprintf "unknown example: %s\n" name;
      exit 2

let () =
  match Sys.argv with
  | [| _; "write"; dir |] -> List.iter (write_one dir) Ir_corpus.examples
  | [| _; "emit"; name |] -> emit_one name
  | [| _; "check"; dir |] ->
      let ok =
        List.fold_left
          (fun acc ex -> check_one dir ex && acc)
          true Ir_corpus.examples
      in
      if not ok then exit 1
  | _ ->
      prerr_endline "usage: dump_fixtures (write|check) <dir>";
      exit 2
