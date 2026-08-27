(* What every protocol suite needs to drive the built server over a pipe:
   framing, a one-shot session that feeds it a script and collects every
   frame it wrote, and the bodies every session starts and ends with. *)

let exe = "../tono_lsp.exe"

let frame body =
  Printf.sprintf "Content-Length: %d\r\n\r\n%s" (String.length body) body

let parse_frames (s : string) : Yojson.Safe.t list =
  let find_sub needle from =
    let n = String.length s and m = String.length needle in
    let rec go i =
      if i + m > n then None
      else if String.sub s i m = needle then Some i
      else go (i + 1)
    in
    go from
  in
  let rec collect i acc =
    match find_sub "Content-Length:" i with
    | None -> List.rev acc
    | Some h -> (
        match find_sub "\r\n\r\n" h with
        | None -> List.rev acc
        | Some sep ->
            let header = String.sub s (h + 15) (sep - h - 15) in
            let len =
              int_of_string
                (String.trim
                   (List.hd (String.split_on_char '\r' (String.trim header))))
            in
            let body = String.sub s (sep + 4) len in
            collect (sep + 4 + len) (Yojson.Safe.from_string body :: acc))
  in
  collect 0 []

(* Send [bodies] (the last one must be the exit notification so the server
   terminates on its own), then return every JSON payload the server wrote
   plus its exit status. *)
let session (bodies : string list) : Yojson.Safe.t list * Unix.process_status =
  let ic, oc = Unix.open_process exe in
  List.iter (fun b -> output_string oc (frame b)) bodies;
  flush oc;
  let buf = Buffer.create 4096 in
  (try
     while true do
       Buffer.add_channel buf ic 1
     done
   with End_of_file -> ());
  let status = Unix.close_process (ic, oc) in
  (parse_frames (Buffer.contents buf), status)

let member name json =
  match json with `Assoc fields -> List.assoc_opt name fields | _ -> None

let response_for (id : int) (frames : Yojson.Safe.t list) : Yojson.Safe.t option
    =
  List.find_opt (fun f -> member "id" f = Some (`Int id)) frames

let exited_cleanly = function Unix.WEXITED 0 -> true | _ -> false
let shutdown_body = {|{"jsonrpc":"2.0","id":99,"method":"shutdown"}|}
let read_file path = In_channel.with_open_bin path In_channel.input_all

let write_file path text =
  Out_channel.with_open_bin path (fun oc -> Out_channel.output_string oc text)

let init_body =
  {|{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"capabilities":{}}}|}

let exit_body = {|{"jsonrpc":"2.0","method":"exit"}|}
