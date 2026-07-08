(* The tono language server: a synchronous stdio JSON-RPC loop over the frontend.
   All the analysis lives in [Analysis] (pure, tested); this module only owns the
   transport, the open-document store, and the request/notification dispatch.

   Concurrency is deliberately absent: requests are served in receive order on a
   single thread. Editors tolerate this for a fast, in-process checker, and it
   keeps the loop trivial to reason about. *)

open Lsp.Types
module CN = Lsp.Client_notification
module CR = Lsp.Client_request
module SN = Lsp.Server_notification
module Text_document = Lsp.Text_document
module Analysis = Tono_lsp_lib.Analysis

(* [Lsp.Io.Make] is parameterized over an IO monad and a channel pair. The
   identity monad turns its [read]/[write] into ordinary blocking calls. *)
module Io = struct
  type 'a t = 'a

  let return x = x
  let raise = raise

  module O = struct
    let ( let+ ) x f = f x
    let ( let* ) x f = f x
  end
end

module Chan = struct
  type input = in_channel
  type output = out_channel

  let read_line ic = try Some (input_line ic) with End_of_file -> None

  let read_exactly ic n =
    let b = Bytes.create n in
    try
      really_input ic b 0 n;
      Some (Bytes.unsafe_to_string b)
    with End_of_file -> None

  let write oc parts =
    List.iter (output_string oc) parts;
    flush oc
end

module Rpc = Lsp.Io.Make (Io) (Chan)

(* Documents are keyed by their stringified URI: [DocumentUri.t] is abstract, so
   a stable string is the simplest hashtable key. *)
let store : (string, Text_document.t) Hashtbl.t = Hashtbl.create 16
let key (uri : DocumentUri.t) : string = Lsp.Uri.to_string uri

let send_notification (n : SN.t) : unit =
  Rpc.write stdout (Jsonrpc.Packet.Notification (SN.to_jsonrpc n))

(* Recompute diagnostics for a document and push them to the client. Called on
   every open and change so the editor's problem list tracks the buffer. *)
let publish_diagnostics (uri : DocumentUri.t) (text : string) : unit =
  let diagnostics = Analysis.lsp_diagnostics text in
  send_notification
    (SN.PublishDiagnostics
       (PublishDiagnosticsParams.create ~uri ~diagnostics ()))

let server_capabilities () : ServerCapabilities.t =
  let sync =
    TextDocumentSyncOptions.create ~openClose:true
      ~change:TextDocumentSyncKind.Incremental ()
  in
  ServerCapabilities.create ~textDocumentSync:(`TextDocumentSyncOptions sync)
    ~hoverProvider:(`Bool true) ~definitionProvider:(`Bool true)
    ~completionProvider:(CompletionOptions.create ())
    ~referencesProvider:(`Bool true) ~documentSymbolProvider:(`Bool true)
    ~renameProvider:(`Bool true) ~documentFormattingProvider:(`Bool true) ()

let initialize_result () : InitializeResult.t =
  let serverInfo =
    InitializeResult.create_serverInfo ~name:"tono-lsp"
      ~version:Tono_frontend.version ()
  in
  InitializeResult.create ~capabilities:(server_capabilities ()) ~serverInfo ()

let doc_text uri =
  Option.map Text_document.text (Hashtbl.find_opt store (key uri))

(* Dispatch a single request. The GADT ties each constructor to its response
   type, so [handle] is written with a locally-abstract result type and the
   compiler checks each arm returns the right shape. Unsupported methods raise a
   MethodNotFound error the caller turns into a JSON-RPC error response. *)
let on_request (req : Jsonrpc.Request.t) : Jsonrpc.Response.t =
  match CR.of_jsonrpc req with
  | Error message ->
      Jsonrpc.Response.error req.id
        (Jsonrpc.Response.Error.make ~code:InvalidRequest ~message ())
  | Ok (CR.E r) -> (
      let handle : type resp. resp CR.t -> resp = function
        | CR.Initialize _ -> initialize_result ()
        | CR.Shutdown -> ()
        | CR.TextDocumentHover p -> (
            match doc_text p.textDocument.uri with
            | Some text ->
                Analysis.hover_at ~text ~file:(Analysis.parse text) p.position
            | None -> None)
        | CR.TextDocumentDefinition p -> (
            match doc_text p.textDocument.uri with
            | Some text -> (
                match
                  Analysis.definition_at ~uri:p.textDocument.uri ~text
                    ~file:(Analysis.parse text) p.position
                with
                | Some loc -> Some (`Location [ loc ])
                | None -> None)
            | None -> None)
        | CR.TextDocumentCompletion p -> (
            match doc_text p.textDocument.uri with
            | Some text ->
                Some (`List (Analysis.completions ~file:(Analysis.parse text)))
            | None -> Some (`List []))
        | CR.TextDocumentReferences p -> (
            match doc_text p.textDocument.uri with
            | Some text ->
                Some
                  (Analysis.references_at ~uri:p.textDocument.uri ~text
                     ~file:(Analysis.parse text)
                     ~include_decl:p.context.includeDeclaration p.position)
            | None -> None)
        | CR.DocumentSymbol p -> (
            match doc_text p.textDocument.uri with
            | Some text ->
                Some
                  (`DocumentSymbol
                     (Analysis.document_symbols ~file:(Analysis.parse text)))
            | None -> None)
        | CR.TextDocumentRename p -> (
            match doc_text p.textDocument.uri with
            | Some text ->
                Analysis.rename_at ~uri:p.textDocument.uri ~text
                  ~file:(Analysis.parse text) ~new_name:p.newName p.position
            | None -> WorkspaceEdit.create ())
        | CR.TextDocumentFormatting p -> (
            match doc_text p.textDocument.uri with
            | Some text -> Analysis.formatting ~text
            | None -> None)
        | _ ->
            Jsonrpc.Response.Error.raise
              (Jsonrpc.Response.Error.make ~code:MethodNotFound
                 ~message:"unsupported request" ())
      in
      try Jsonrpc.Response.ok req.id (CR.yojson_of_result r (handle r))
      with Jsonrpc.Response.Error.E e -> Jsonrpc.Response.error req.id e)

(* Notifications never get a reply; document-sync ones refresh diagnostics. *)
let on_notification (n : Jsonrpc.Notification.t) : unit =
  match CN.of_jsonrpc n with
  | Error _ -> ()
  | Ok (CN.TextDocumentDidOpen p) ->
      let doc = Text_document.make ~position_encoding:`UTF8 p in
      Hashtbl.replace store (key p.textDocument.uri) doc;
      publish_diagnostics p.textDocument.uri (Text_document.text doc)
  | Ok (CN.TextDocumentDidChange p) -> (
      match Hashtbl.find_opt store (key p.textDocument.uri) with
      | Some doc ->
          let doc = Text_document.apply_content_changes doc p.contentChanges in
          Hashtbl.replace store (key p.textDocument.uri) doc;
          publish_diagnostics p.textDocument.uri (Text_document.text doc)
      | None -> ())
  | Ok (CN.TextDocumentDidClose p) ->
      Hashtbl.remove store (key p.textDocument.uri)
  | Ok _ -> ()

let () =
  set_binary_mode_in stdin true;
  set_binary_mode_out stdout true;
  let running = ref true in
  while !running do
    match Rpc.read stdin with
    | None -> running := false
    | Some (Jsonrpc.Packet.Request req) ->
        Rpc.write stdout (Jsonrpc.Packet.Response (on_request req))
    | Some (Jsonrpc.Packet.Notification n) -> (
        (* [exit] terminates the loop; every other notification is handled. *)
        match CN.of_jsonrpc n with
        | Ok CN.Exit -> running := false
        | _ -> on_notification n)
    | Some _ -> ()
  done
