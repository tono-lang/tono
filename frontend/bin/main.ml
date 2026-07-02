let usage () =
  prerr_endline "usage: tono fmt [-w] <file>...";
  prerr_endline "       tono version";
  exit 2

let read_file path =
  let ic = open_in_bin path in
  Fun.protect
    ~finally:(fun () -> close_in_noerr ic)
    (fun () -> really_input_string ic (in_channel_length ic))

(* Format one file: print the canonical form to stdout, or rewrite the file in
   place with [-w]. A file that parses with errors is reported and left alone. *)
let fmt_file ~write path =
  let src = read_file path in
  let file, diags = Tono_frontend.Parser.parse src in
  let errors =
    List.filter
      (fun (d : Tono_frontend.Diagnostic.t) ->
        d.severity = Tono_frontend.Diagnostic.Error)
      diags
  in
  if errors <> [] then (
    List.iter
      (fun d ->
        Printf.eprintf "%s:%s\n" path (Tono_frontend.Diagnostic.to_string d))
      errors;
    false)
  else
    let out = Tono_frontend.Printer.print_file file in
    if write then
      let oc = open_out_bin path in
      Fun.protect
        ~finally:(fun () -> close_out_noerr oc)
        (fun () -> output_string oc out)
    else print_string out;
    true

let () =
  match Array.to_list Sys.argv with
  | [ _ ] | [ _; "version" ] -> print_endline ("tono " ^ Tono_frontend.version)
  | _ :: "fmt" :: rest -> (
      let write, files =
        match rest with "-w" :: fs -> (true, fs) | fs -> (false, fs)
      in
      match files with
      | [] -> usage ()
      | files ->
          let ok =
            List.fold_left (fun acc f -> fmt_file ~write f && acc) true files
          in
          exit (if ok then 0 else 1))
  | _ -> usage ()
