(* The `tono-frontend` binary: a thin shell over [Tono_frontend.Cli.run], which
   holds the testable dispatch. Here we only supply the real file reader and wire
   the outcome to the process (stdout, stderr, exit code). *)

let read_file path =
  let ic = open_in_bin path in
  Fun.protect
    ~finally:(fun () -> close_in ic)
    (fun () -> really_input_string ic (in_channel_length ic))

(* Every file under [root], as paths relative to it, walking subdirectories so a
   nested layout (payments/charge.tono) maps to a dotted module name. *)
let list_files root =
  let rec walk rel acc =
    let dir = if rel = "" then root else Filename.concat root rel in
    Array.fold_left
      (fun acc entry ->
        let rel' = if rel = "" then entry else Filename.concat rel entry in
        let full = Filename.concat root rel' in
        if Sys.is_directory full then walk rel' acc else rel' :: acc)
      acc (Sys.readdir dir)
  in
  walk "" []

let () =
  let outcome = Tono_frontend.Cli.run ~list_files ~read_file Sys.argv in
  print_string outcome.Tono_frontend.Cli.out;
  prerr_string outcome.Tono_frontend.Cli.err;
  exit outcome.Tono_frontend.Cli.code
