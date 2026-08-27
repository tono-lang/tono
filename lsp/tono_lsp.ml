(* The tono language server: a synchronous stdio JSON-RPC loop over the frontend.
   All the analysis lives in [Analysis] (pure, tested); this module only owns the
   transport, the open-document store, and the request/notification dispatch.

   Requests are served in receive order on a single thread: editors tolerate
   this for a fast, in-process checker, and it keeps the loop trivial to
   reason about. The one thing off that thread is the foreign-binding check
   on save, which invokes the target compilers ([Binding_runner]): its
   workers publish through the same guarded writer, and the document store
   is read under a lock wherever a worker can reach it. *)

open Lsp.Types
module CN = Lsp.Client_notification
module CR = Lsp.Client_request
module SN = Lsp.Server_notification
module Text_document = Lsp.Text_document
module Analysis = Tono_lsp_lib.Analysis
module Project = Tono_lsp_lib.Project

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

(* The store and the last frontend diagnostics per document are read by the
   binding-check workers when they publish; every write to them from the
   request thread and every read from a worker holds this lock. *)
let state_lock = Mutex.create ()

let with_state f =
  Mutex.lock state_lock;
  Fun.protect ~finally:(fun () -> Mutex.unlock state_lock) f

(* Frames are written whole: the request thread and the workers share the
   channel, and an interleaved frame would end the session. *)
let out_lock = Mutex.create ()

let write_packet (p : Jsonrpc.Packet.t) : unit =
  Mutex.lock out_lock;
  Fun.protect
    ~finally:(fun () -> Mutex.unlock out_lock)
    (fun () -> Rpc.write stdout p)

let send_notification (n : SN.t) : unit =
  write_packet (Jsonrpc.Packet.Notification (SN.to_jsonrpc n))

let log_message (message : string) : unit =
  send_notification
    (SN.LogMessage (LogMessageParams.create ~type_:MessageType.Info ~message))

(* The frontend's diagnostics as last published per document. What the
   editor sees is these plus the binding verdicts cached for the file, so a
   keystroke re-publishing the frontend's never wipes what the last save
   found, and a save's verdict lands on top of what typing shows. *)
let frontend_diags : (string, Diagnostic.t list) Hashtbl.t = Hashtbl.create 16

let has_error (ds : Diagnostic.t list) : bool =
  List.exists
    (fun (d : Diagnostic.t) -> d.severity = Some DiagnosticSeverity.Error)
    ds

(* Publish the merged diagnostics of [uri]: the frontend's last, then the
   binding verdicts re-located on the buffer as it is now. A buffer the
   frontend rejects shows the frontend's alone (the check needs a source it
   accepts, and a verdict about a file that no longer compiles misleads). *)
let publish_merged (uri : DocumentUri.t) : unit =
  let diagnostics =
    with_state (fun () ->
        let k = key uri in
        let fd = Option.value ~default:[] (Hashtbl.find_opt frontend_diags k) in
        let bd =
          match Hashtbl.find_opt store k with
          | Some doc when not (has_error fd) ->
              Binding_runner.diagnostics ~path:(Lsp.Uri.to_path uri)
                ~text:(Text_document.text doc)
          | _ -> []
        in
        fd @ bd)
  in
  send_notification
    (SN.PublishDiagnostics
       (PublishDiagnosticsParams.create ~uri ~diagnostics ()))

let publish (uri : DocumentUri.t) (diagnostics : Diagnostic.t list) : unit =
  with_state (fun () -> Hashtbl.replace frontend_diags (key uri) diagnostics);
  publish_merged uri

(* Recompute diagnostics for a document and push them to the client. Called on
   every open and change so the editor's problem list tracks the buffer. *)
let publish_diagnostics (uri : DocumentUri.t) (text : string) : unit =
  publish uri (Analysis.lsp_diagnostics text)

let server_capabilities () : ServerCapabilities.t =
  let sync =
    TextDocumentSyncOptions.create ~openClose:true
      ~change:TextDocumentSyncKind.Incremental
      ~save:(`SaveOptions (SaveOptions.create ~includeText:false ()))
      ()
  in
  ServerCapabilities.create ~textDocumentSync:(`TextDocumentSyncOptions sync)
    ~hoverProvider:(`Bool true) ~definitionProvider:(`Bool true)
    ~completionProvider:(CompletionOptions.create ())
    ~referencesProvider:(`Bool true) ~documentSymbolProvider:(`Bool true)
    ~renameProvider:(`Bool true) ~documentFormattingProvider:(`Bool true)
    ~codeActionProvider:(`Bool true) ~workspaceSymbolProvider:(`Bool true)
    ~signatureHelpProvider:
      (SignatureHelpOptions.create ~triggerCharacters:[ "("; "," ] ())
    ()

let initialize_result () : InitializeResult.t =
  let serverInfo =
    InitializeResult.create_serverInfo ~name:"tono-lsp"
      ~version:Tono_frontend.version ()
  in
  InitializeResult.create ~capabilities:(server_capabilities ()) ~serverInfo ()

let doc_text uri =
  Option.map Text_document.text (Hashtbl.find_opt store (key uri))

(* The lifecycle phase gates dispatch: before initialize every request is
   answered ServerNotInitialized and every notification except exit is
   dropped; after shutdown every request is an error and exit is the only
   notification that matters. Exit without a prior shutdown is the abnormal
   path and reports exit code 1. *)
type phase = Uninitialized | Running | ShutDown

let phase = ref Uninitialized

(* Whether the client renders markdown hover, learned at initialize; picks the
   hover content flavor for the whole session. *)
let client_markdown = ref false

let remember_markdown (caps : ClientCapabilities.t) : unit =
  client_markdown :=
    match
      Option.bind caps.textDocument
        (fun (td : TextDocumentClientCapabilities.t) ->
          Option.bind td.hover (fun (h : HoverClientCapabilities.t) ->
              h.contentFormat))
    with
    | Some formats -> List.mem MarkupKind.Markdown formats
    | None -> false

(* --- workspace wiring --- *)

(* Whether the client accepts dynamic registration for file watchers, learned
   at initialize; the watcher request goes out on [initialized]. *)
let client_watch_registration = ref false

let remember_watch_capability (caps : ClientCapabilities.t) : unit =
  client_watch_registration :=
    Option.value ~default:false
      (Option.bind caps.workspace (fun (w : WorkspaceClientCapabilities.t) ->
           Option.bind w.didChangeWatchedFiles
             (fun (c : DidChangeWatchedFilesClientCapabilities.t) ->
               c.dynamicRegistration)))

(* Open documents that belong to a project: uri key -> (source root, module). *)
let doc_roots : (string, string * string) Hashtbl.t = Hashtbl.create 16

(* The open-buffer overlay as (path, text) pairs: unsaved editor content wins
   over disk, and a buffer with no file on disk still joins its project. *)
let open_docs () : (string * string) list =
  Hashtbl.fold
    (fun _ doc acc ->
      (Lsp.Uri.to_path (Text_document.documentUri doc), Text_document.text doc)
      :: acc)
    store []

let project_ctx (uri : DocumentUri.t) : (Project.project * string) option =
  Option.map
    (fun (root, module_) ->
      (Workspace.project_of ~open_docs:(open_docs ()) root, module_))
    (Hashtbl.find_opt doc_roots (key uri))

(* Re-publish every open document under [root]: an edit in one file can change
   its dependents' diagnostics, and the per-module cache makes untouched
   modules free. *)
let publish_project_root (root : string) : unit =
  let project = Workspace.project_of ~open_docs:(open_docs ()) root in
  Hashtbl.iter
    (fun k (r, module_) ->
      if String.equal r root then
        publish (Lsp.Uri.of_string k)
          (Workspace.diagnostics ~project ~root ~module_))
    doc_roots

let republish_all_roots () : unit =
  let roots =
    Hashtbl.fold
      (fun _ (root, _) acc -> if List.mem root acc then acc else root :: acc)
      doc_roots []
  in
  List.iter publish_project_root roots

(* Analyze one document: with a discoverable project root the diagnostics
   carry full project context; otherwise the single-file path. *)
let analyze_and_publish (uri : DocumentUri.t) (text : string) : unit =
  let path = Lsp.Uri.to_path uri in
  match Workspace.source_root_of path with
  | Some root when Workspace.module_of ~root path <> None -> (
      match Workspace.module_of ~root path with
      | Some module_ ->
          Hashtbl.replace doc_roots (key uri) (root, module_);
          publish_project_root root
      | None -> publish_diagnostics uri text)
  | _ ->
      Hashtbl.remove doc_roots (key uri);
      publish_diagnostics uri text

(* Ask the client to watch .tono files so an edit to an imported file on disk
   refreshes open dependents. *)
let register_watcher () : unit =
  let watchers =
    [ FileSystemWatcher.create ~globPattern:(`Pattern "**/*.tono") () ]
  in
  let options = DidChangeWatchedFilesRegistrationOptions.create ~watchers in
  let registration =
    Registration.create ~id:"tono.watched-files"
      ~method_:"workspace/didChangeWatchedFiles"
      ~registerOptions:
        (DidChangeWatchedFilesRegistrationOptions.yojson_of_t options)
      ()
  in
  let params = RegistrationParams.create ~registrations:[ registration ] in
  write_packet
    (Jsonrpc.Packet.Request
       (Jsonrpc.Request.create ~id:(`String "tono.watched-files.registration")
          ~method_:"client/registerCapability"
          ~params:
            (Jsonrpc.Structured.t_of_yojson
               (RegistrationParams.yojson_of_t params))
          ()))

(* Dispatch a single request. The GADT ties each constructor to its response
   type, so [handle] is written with a locally-abstract result type and the
   compiler checks each arm returns the right shape. Unsupported methods raise a
   MethodNotFound error the caller turns into a JSON-RPC error response. *)
let on_request (req : Jsonrpc.Request.t) : Jsonrpc.Response.t =
  match !phase with
  | Uninitialized when req.method_ <> "initialize" ->
      Jsonrpc.Response.error req.id
        (Jsonrpc.Response.Error.make ~code:ServerNotInitialized
           ~message:"server not initialized" ())
  | ShutDown ->
      Jsonrpc.Response.error req.id
        (Jsonrpc.Response.Error.make ~code:InvalidRequest
           ~message:"server is shut down" ())
  | _ -> (
      match CR.of_jsonrpc req with
      | Error message ->
          Jsonrpc.Response.error req.id
            (Jsonrpc.Response.Error.make ~code:InvalidRequest ~message ())
      | Ok (CR.E r) -> (
          let handle : type resp. resp CR.t -> resp = function
            | CR.Initialize p ->
                if !phase <> Uninitialized then
                  Jsonrpc.Response.Error.raise
                    (Jsonrpc.Response.Error.make ~code:InvalidRequest
                       ~message:"initialize may only be sent once" ());
                remember_markdown p.capabilities;
                remember_watch_capability p.capabilities;
                phase := Running;
                initialize_result ()
            | CR.Shutdown ->
                phase := ShutDown;
                ()
            | CR.TextDocumentHover p -> (
                match doc_text p.textDocument.uri with
                | Some text ->
                    Analysis.hover_at ~markdown:!client_markdown ~text
                      ~file:(Analysis.parse text) p.position
                | None -> None)
            | CR.TextDocumentDefinition p -> (
                match project_ctx p.textDocument.uri with
                | Some (project, module_) -> (
                    match
                      Project.project_symbol_at project ~module_ p.position
                    with
                    | Some (m, name) -> (
                        match
                          Project.project_decl_location project ~module_:m ~name
                        with
                        | Some (id, range) ->
                            Some
                              (`Location
                                 [
                                   Location.create ~uri:(Lsp.Uri.of_string id)
                                     ~range;
                                 ])
                        | None -> None)
                    | None -> None)
                | None -> (
                    match doc_text p.textDocument.uri with
                    | Some text -> (
                        match
                          Analysis.definition_at ~uri:p.textDocument.uri ~text
                            ~file:(Analysis.parse text) p.position
                        with
                        | Some loc -> Some (`Location [ loc ])
                        | None -> None)
                    | None -> None))
            | CR.TextDocumentCompletion p -> (
                match doc_text p.textDocument.uri with
                | Some text ->
                    Some
                      (`List
                         (Analysis.completions ~text ~file:(Analysis.parse text)
                            p.position))
                | None -> Some (`List []))
            | CR.TextDocumentReferences p -> (
                match project_ctx p.textDocument.uri with
                | Some (project, module_) -> (
                    match
                      Project.project_symbol_at project ~module_ p.position
                    with
                    | Some (m, name) ->
                        Some
                          (List.map
                             (fun (id, text, sp) ->
                               Location.create ~uri:(Lsp.Uri.of_string id)
                                 ~range:(Analysis.range_in ~text sp))
                             (Project.project_occurrences project ~module_:m
                                ~name ~include_decl:p.context.includeDeclaration))
                    | None -> Some [])
                | None -> (
                    match doc_text p.textDocument.uri with
                    | Some text ->
                        Some
                          (Analysis.references_at ~uri:p.textDocument.uri ~text
                             ~file:(Analysis.parse text)
                             ~include_decl:p.context.includeDeclaration
                             p.position)
                    | None -> None))
            | CR.DocumentSymbol p -> (
                match doc_text p.textDocument.uri with
                | Some text ->
                    Some
                      (`DocumentSymbol
                         (Analysis.document_symbols ~text
                            ~file:(Analysis.parse text)))
                | None -> None)
            | CR.TextDocumentRename p -> (
                match project_ctx p.textDocument.uri with
                | Some (project, module_) -> (
                    match
                      Project.project_rename project ~module_ p.position
                        ~new_name:p.newName
                    with
                    | Project.Renamed changes ->
                        WorkspaceEdit.create
                          ~changes:
                            (List.map
                               (fun (id, edits) ->
                                 (Lsp.Uri.of_string id, edits))
                               changes)
                          ()
                    | Project.Refused message ->
                        Jsonrpc.Response.Error.raise
                          (Jsonrpc.Response.Error.make ~code:InvalidParams
                             ~message ())
                    | Project.NotASymbol -> WorkspaceEdit.create ())
                | None -> (
                    if not (Analysis.valid_identifier p.newName) then
                      Jsonrpc.Response.Error.raise
                        (Jsonrpc.Response.Error.make ~code:InvalidParams
                           ~message:
                             (Printf.sprintf
                                "'%s' is not a valid declaration name" p.newName)
                           ());
                    match doc_text p.textDocument.uri with
                    | Some text ->
                        Analysis.rename_at ~uri:p.textDocument.uri ~text
                          ~file:(Analysis.parse text) ~new_name:p.newName
                          p.position
                    | None -> WorkspaceEdit.create ()))
            | CR.TextDocumentFormatting p -> (
                match doc_text p.textDocument.uri with
                | Some text -> Analysis.formatting ~text
                | None -> None)
            | CR.CodeAction p -> (
                match project_ctx p.textDocument.uri with
                | None -> None
                | Some (project, module_) ->
                    Some
                      (List.map
                         (fun (title, edits) ->
                           let changes =
                             List.map
                               (fun (id, e) -> (Lsp.Uri.of_string id, [ e ]))
                               edits
                           in
                           `CodeAction
                             (CodeAction.create ~title
                                ~kind:CodeActionKind.QuickFix
                                ~edit:(WorkspaceEdit.create ~changes ())
                                ()))
                         (Project.project_code_actions project ~module_
                            ~range:p.range)))
            | CR.WorkspaceSymbol p ->
                let roots =
                  Hashtbl.fold
                    (fun _ (root, _) acc ->
                      if List.mem root acc then acc else root :: acc)
                    doc_roots []
                in
                Some
                  (List.concat_map
                     (fun root ->
                       let project =
                         Workspace.project_of ~open_docs:(open_docs ()) root
                       in
                       List.map
                         (fun (name, kind, id, range) ->
                           SymbolInformation.create ~name ~kind
                             ~location:
                               (Location.create ~uri:(Lsp.Uri.of_string id)
                                  ~range)
                             ())
                         (Project.project_symbols project ~query:p.query))
                     roots)
            | CR.SignatureHelp p -> (
                match doc_text p.textDocument.uri with
                | Some text -> (
                    match
                      Project.signature_help ~text ~file:(Analysis.parse text)
                        p.position
                    with
                    | Some sh -> sh
                    | None -> SignatureHelp.create ~signatures:[] ())
                | None -> SignatureHelp.create ~signatures:[] ())
            | _ ->
                Jsonrpc.Response.Error.raise
                  (Jsonrpc.Response.Error.make ~code:MethodNotFound
                     ~message:"unsupported request" ())
          in
          try Jsonrpc.Response.ok req.id (CR.yojson_of_result r (handle r))
          with Jsonrpc.Response.Error.E e -> Jsonrpc.Response.error req.id e))

(* Notifications never get a reply; document-sync ones refresh diagnostics. *)
let on_notification (n : Jsonrpc.Notification.t) : unit =
  match CN.of_jsonrpc n with
  | Error _ -> ()
  | Ok (CN.TextDocumentDidOpen p) ->
      (* UTF16 matches what clients send: LSP positions default to UTF-16 code
         units, and incremental didChange ranges applied at any other encoding
         desynchronize the buffer on non-ASCII content. *)
      let doc = Text_document.make ~position_encoding:`UTF16 p in
      with_state (fun () -> Hashtbl.replace store (key p.textDocument.uri) doc);
      analyze_and_publish p.textDocument.uri (Text_document.text doc)
  | Ok (CN.TextDocumentDidChange p) -> (
      match Hashtbl.find_opt store (key p.textDocument.uri) with
      | Some doc ->
          let doc = Text_document.apply_content_changes doc p.contentChanges in
          with_state (fun () ->
              Hashtbl.replace store (key p.textDocument.uri) doc);
          analyze_and_publish p.textDocument.uri (Text_document.text doc)
      | None -> ())
  | Ok (CN.DidSaveTextDocument p) -> (
      (* The saved text is the buffer's (the client sends no text, and the
         check reads the file from disk, which the save just wrote). *)
      match Hashtbl.find_opt store (key p.textDocument.uri) with
      | Some doc ->
          let uri = p.textDocument.uri in
          let clean =
            not
              (has_error
                 (Option.value ~default:[]
                    (Hashtbl.find_opt frontend_diags (key uri))))
          in
          Binding_runner.schedule ~path:(Lsp.Uri.to_path uri)
            ~text:(Text_document.text doc) ~clean ~log:log_message
            ~on_done:(fun () -> publish_merged uri)
      | None -> ())
  | Ok (CN.TextDocumentDidClose p) ->
      with_state (fun () ->
          Hashtbl.remove store (key p.textDocument.uri);
          Hashtbl.remove frontend_diags (key p.textDocument.uri));
      Hashtbl.remove doc_roots (key p.textDocument.uri);
      Binding_runner.forget ~path:(Lsp.Uri.to_path p.textDocument.uri);
      (* Editors keep published diagnostics until told otherwise; clear them so
         a closed buffer leaves no stale problems behind. *)
      send_notification
        (SN.PublishDiagnostics
           (PublishDiagnosticsParams.create ~uri:p.textDocument.uri
              ~diagnostics:[] ()))
  | Ok CN.Initialized -> if !client_watch_registration then register_watcher ()
  | Ok (CN.DidChangeWatchedFiles _) ->
      (* An imported file changed or vanished on disk: dependents among the
         open documents get fresh diagnostics (the overlay still wins for open
         buffers). *)
      republish_all_roots ()
  | Ok _ -> ()

(* One inbound frame, read with the raw channel primitives instead of
   [Rpc.read]: that helper raises on a malformed JSON body after the bytes are
   consumed, which would kill the loop with no way to answer. Parsing here
   keeps the raw json around, so a broken request (e.g. `"params": null`,
   which JSON-RPC forbids) still gets an error response keyed by its id, and
   the server survives whatever a client sends. *)
type frame = Eof | Packet of Jsonrpc.Packet.t | Broken of Jsonrpc.Id.t option

let read_frame ic : frame =
  let rec content_length len =
    match Chan.read_line ic with
    | None -> None
    | Some line -> (
        let line = String.trim line in
        if line = "" then len
        else
          match String.index_opt line ':' with
          | Some i
            when String.lowercase_ascii (String.sub line 0 i) = "content-length"
            ->
              content_length
                (int_of_string_opt
                   (String.trim
                      (String.sub line (i + 1) (String.length line - i - 1))))
          | _ -> content_length len)
  in
  match content_length None with
  | None -> Eof
  | Some n -> (
      match Chan.read_exactly ic n with
      | None -> Eof
      | Some body -> (
          match Yojson.Safe.from_string body with
          | exception _ -> Broken None
          | json -> (
              match Jsonrpc.Packet.t_of_yojson json with
              | packet -> Packet packet
              | exception _ ->
                  let id =
                    match json with
                    | `Assoc fields -> (
                        match List.assoc_opt "id" fields with
                        | Some (`Int i) -> Some (`Int i)
                        | Some (`String s) -> Some (`String s)
                        | _ -> None)
                    | _ -> None
                  in
                  Broken id)))

let error_response id code message =
  Jsonrpc.Packet.Response
    (Jsonrpc.Response.error id (Jsonrpc.Response.Error.make ~code ~message ()))

let () =
  set_binary_mode_in stdin true;
  set_binary_mode_out stdout true;
  let running = ref true in
  let dirty_exit = ref false in
  while !running do
    match read_frame stdin with
    | Eof -> running := false
    | Broken (Some id) ->
        write_packet
          (error_response id InvalidParams
             "request params must be a structured value")
    | Broken None -> ()
    | Packet (Jsonrpc.Packet.Request req) ->
        (* A handler bug must answer as an error, never take the server down:
           the editor session outlives any single request. *)
        let response =
          try on_request req
          with exn ->
            Jsonrpc.Response.error req.id
              (Jsonrpc.Response.Error.make ~code:InternalError
                 ~message:(Printexc.to_string exn) ())
        in
        write_packet (Jsonrpc.Packet.Response response)
    | Packet (Jsonrpc.Packet.Notification n) -> (
        (* [exit] terminates the loop; every other notification is handled
           only while running (dropped before initialize and after shutdown),
           and a failure inside one is dropped (notifications have no
           reply). *)
        match CN.of_jsonrpc n with
        | Ok CN.Exit ->
            if !phase <> ShutDown then dirty_exit := true;
            running := false
        | _ when !phase <> Running -> ()
        | _ -> ( try on_notification n with _ -> ()))
    | Packet _ -> ()
  done;
  (* A verdict still being computed is published before the session ends:
     the client asked for it with its save. *)
  Binding_runner.wait_all ();
  if !dirty_exit then exit 1
