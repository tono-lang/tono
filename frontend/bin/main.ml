(* The `tono-frontend` binary: a thin shell over [Tono_frontend.Cli.run], which
   holds the testable dispatch. Here we only supply the real file reader and wire
   the outcome to the process (stdout, stderr, exit code). *)

let read_file path =
  let ic = open_in_bin path in
  Fun.protect
    ~finally:(fun () -> close_in ic)
    (fun () -> really_input_string ic (in_channel_length ic))

let () =
  let outcome = Tono_frontend.Cli.run ~read_file Sys.argv in
  print_string outcome.Tono_frontend.Cli.out;
  prerr_string outcome.Tono_frontend.Cli.err;
  exit outcome.Tono_frontend.Cli.code
