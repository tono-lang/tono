//! The request-assembly half of the emitted `internal/transport` package:
//! the helpers the generated call sites name to build a URL, layer headers,
//! and encode a body, plus the raw dispatch behind one attempt. Split from
//! [`super::send`] (which owns the call policy: Send, retry, timeout, hooks)
//! along that seam, so each half stays readable on its own.

use crate::codegen::tree::Decl;

use super::send::decl_table;
use super::{import, support_symbol};

pub(super) fn dispatch_decl() -> Decl {
    Decl::raw_providing(
        "dispatch",
        "// dispatch performs the raw exchange: the canonical transport when set, the\n\
         // native client otherwise (http.DefaultClient when neither).\n\
         func dispatch(ctx context.Context, native *http.Client, canonical support.HTTPTransport, req support.HTTPRequest) (support.HTTPResponse, error) {\n\
         \tif canonical != nil {\n\t\treturn canonical(ctx, req)\n\t}\n\
         \tclient := native\n\
         \tif client == nil {\n\t\tclient = http.DefaultClient\n\t}\n\
         \tvar reader io.Reader\n\
         \tif req.Body != nil {\n\t\treader = bytes.NewReader(req.Body)\n\t}\n\
         \thttpReq, err := http.NewRequestWithContext(ctx, req.Method, req.URL, reader)\n\
         \tif err != nil {\n\t\treturn support.HTTPResponse{}, err\n\t}\n\
         \tfor name, value := range req.Headers {\n\t\thttpReq.Header.Set(name, value)\n\t}\n\
         \thttpRes, err := client.Do(httpReq)\n\
         \tif err != nil {\n\t\treturn support.HTTPResponse{}, err\n\t}\n\
         \tdefer httpRes.Body.Close()\n\
         \t// The body read can fail mid-stream too, so it shares the transport error\n\
         \t// path.\n\
         \tdata, err := io.ReadAll(httpRes.Body)\n\
         \tif err != nil {\n\t\treturn support.HTTPResponse{}, err\n\t}\n\
         \theaders := make(map[string]string, len(httpRes.Header))\n\
         \tfor name := range httpRes.Header {\n\t\theaders[strings.ToLower(name)] = httpRes.Header.Get(name)\n\t}\n\
         \treturn support.HTTPResponse{Status: httpRes.StatusCode, Headers: headers, Body: string(data)}, nil\n\
         }"
        .to_string(),
        vec![
            import("context", "context"),
            import("http", "net/http"),
            import("io", "io"),
            import("bytes", "bytes"),
            import("strings", "strings"),
            support_symbol("HTTPTransport"),
            support_symbol("HTTPRequest"),
            support_symbol("HTTPResponse"),
        ],
    )
}

/// The request-assembly helpers the generated call sites name directly. Each
/// is its own declaration, so the root-group pruning drops the ones no
/// operation in the SDK reaches.
pub(super) fn assembly_decls() -> Vec<Decl> {
    decl_table(vec![
        (
            "FormatScalar",
            "// FormatScalar renders a value the way the wire expects it in a path,\n\
             // query, or header position. Decoded JSON numbers arrive as float64; an\n\
             // integral one must not print a trailing \".0\".\n\
             func FormatScalar(v any) string {\n\
             \tswitch x := v.(type) {\n\
             \tcase string:\n\t\treturn x\n\
             \tcase bool:\n\t\treturn strconv.FormatBool(x)\n\
             \tcase float64:\n\t\treturn strconv.FormatFloat(x, 'f', -1, 64)\n\
             \tcase int:\n\t\treturn strconv.Itoa(x)\n\
             \tcase int64:\n\t\treturn strconv.FormatInt(x, 10)\n\
             \tdefault:\n\
             \t\tb, err := json.Marshal(v)\n\
             \t\tif err != nil {\n\t\t\treturn \"\"\n\t\t}\n\
             \t\treturn string(b)\n\
             \t}\n\
             }",
            vec![
                import("strconv", "strconv"),
                import("json", "encoding/json"),
            ],
        ),
        (
            "PathPart",
            "// PathPart renders a path segment: an absent value substitutes empty\n\
             // rather than a literal \"null\".\n\
             func PathPart(v any) string {\n\
             \tif v == nil {\n\t\treturn \"\"\n\t}\n\
             \treturn url.PathEscape(FormatScalar(v))\n\
             }",
            vec![import("url", "net/url")],
        ),
        (
            "FormatRaw",
            "// FormatRaw renders a record member's raw JSON value the way the wire\n\
             // expects it in a path, query, or header position: a JSON string\n\
             // unquoted, anything else verbatim, so a wide integer or a\n\
             // formatting-sensitive float keeps the exact spelling its own encoder\n\
             // gave it. An absent or null value renders empty.\n\
             func FormatRaw(raw json.RawMessage) string {\n\
             \tif len(raw) == 0 || string(raw) == \"null\" {\n\t\treturn \"\"\n\t}\n\
             \tif raw[0] == '\"' {\n\
             \t\t// The common case (no escape sequence) slices the quotes off\n\
             \t\t// directly; only a string carrying an escape pays for the decoder.\n\
             \t\tinner := raw[1 : len(raw)-1]\n\
             \t\tif bytes.IndexByte(inner, '\\\\') == -1 {\n\t\t\treturn string(inner)\n\t\t}\n\
             \t\tvar s string\n\
             \t\tif err := json.Unmarshal(raw, &s); err != nil {\n\t\t\treturn \"\"\n\t\t}\n\
             \t\treturn s\n\
             \t}\n\
             \treturn string(raw)\n\
             }",
            vec![
                import("json", "encoding/json"),
                import("bytes", "bytes"),
            ],
        ),
        (
            "PathPartRaw",
            "// PathPartRaw renders a record member's raw JSON value into a path\n\
             // segment: an absent or null value substitutes empty rather than a\n\
             // literal \"null\".\n\
             func PathPartRaw(raw json.RawMessage) string {\n\
             \treturn url.PathEscape(FormatRaw(raw))\n\
             }",
            vec![import("url", "net/url")],
        ),
        (
            "SetHeader",
            "// SetHeader overrides across casings: header names are case-insensitive,\n\
             // so a bespoke \"authorization\" replaces a declared \"Authorization\" rather\n\
             // than riding beside it.\n\
             func SetHeader(headers map[string]string, name, value string) {\n\
             \tfor key := range headers {\n\
             \t\tif strings.EqualFold(key, name) {\n\t\t\tdelete(headers, key)\n\t\t}\n\
             \t}\n\
             \theaders[name] = value\n\
             }",
            vec![import("strings", "strings")],
        ),
        (
            "HasHeader",
            "// HasHeader reports whether a header is already set under any casing: a\n\
             // caller-supplied \"Content-Type\" must suppress the default rather than\n\
             // sit beside a second \"content-type\".\n\
             func HasHeader(headers map[string]string, name string) bool {\n\
             \tfor key := range headers {\n\
             \t\tif strings.EqualFold(key, name) {\n\t\t\treturn true\n\t\t}\n\
             \t}\n\
             \treturn false\n\
             }",
            vec![import("strings", "strings")],
        ),
        (
            "AppendQuery",
            "// AppendQuery serializes a record member's raw JSON value as a repeated\n\
             // entry per element for a list, a single entry otherwise; an absent or\n\
             // null value is omitted, the body's nullable-omit rule applied to the\n\
             // request line. A malformed array binds as a single entry rather than\n\
             // failing the request line.\n\
             func AppendQuery(entries []string, name string, raw json.RawMessage) []string {\n\
             \tif len(raw) == 0 || string(raw) == \"null\" {\n\t\treturn entries\n\t}\n\
             \tadd := func(v json.RawMessage) []string {\n\
             \t\treturn append(entries, url.QueryEscape(name)+\"=\"+url.QueryEscape(FormatRaw(v)))\n\
             \t}\n\
             \tif raw[0] == '[' {\n\
             \t\tvar elements []json.RawMessage\n\
             \t\tif err := json.Unmarshal(raw, &elements); err == nil {\n\
             \t\t\tfor _, element := range elements {\n\t\t\t\tentries = add(element)\n\t\t\t}\n\
             \t\t\treturn entries\n\
             \t\t}\n\
             \t}\n\
             \treturn add(raw)\n\
             }",
            vec![import("url", "net/url"), import("json", "encoding/json")],
        ),
        (
            "QueryString",
            "// QueryString folds the collected query entries into the URL tail: empty\n\
             // when nothing survived the omit rules, \"?\"-prefixed otherwise.\n\
             func QueryString(entries []string) string {\n\
             \tif len(entries) == 0 {\n\t\treturn \"\"\n\t}\n\
             \treturn \"?\" + strings.Join(entries, \"&\")\n\
             }",
            vec![import("strings", "strings")],
        ),
        (
            "EncodeBody",
            "// EncodeBody assembles the body-bound members into a JSON object by\n\
             // concatenating their raw bytes, in the given member order: no member is\n\
             // ever decoded and re-encoded, so a value reaches the wire with the exact\n\
             // spelling and precision its own encoder gave it. A member that is\n\
             // present but null still lands in the object as null; only absence omits\n\
             // it. Nil when no member is present.\n\
             func EncodeBody(record map[string]json.RawMessage, members ...string) []byte {\n\
             \tvar fields []byte\n\
             \tfor _, member := range members {\n\
             \t\traw, ok := record[member]\n\
             \t\tif !ok {\n\t\t\tcontinue\n\t\t}\n\
             \t\t// Marshalling a string cannot fail, so the key carries no error path.\n\
             \t\tkey, _ := json.Marshal(member)\n\
             \t\tif fields == nil {\n\t\t\tfields = append(fields, '{')\n\t\t} else {\n\t\t\tfields = append(fields, ',')\n\t\t}\n\
             \t\tfields = append(fields, key...)\n\
             \t\tfields = append(fields, ':')\n\
             \t\tfields = append(fields, raw...)\n\
             \t}\n\
             \tif fields == nil {\n\t\treturn nil\n\t}\n\
             \treturn append(fields, '}')\n\
             }",
            vec![import("json", "encoding/json")],
        ),
        (
            "FoldResponse",
            "// FoldResponse folds the response-bound members (a header value, or the\n\
             // status code) into the decoded body so the generated decoder sees them as\n\
             // ordinary fields. Applied on the success path only; a non-object or empty\n\
             // body leaves the bound fields to stand on their own. The body's own\n\
             // members are held raw and re-emitted verbatim, so folding a status or\n\
             // header in never rewrites a number the server sent.\n\
             func FoldResponse(body string, bound map[string]json.RawMessage) string {\n\
             \tobject := map[string]json.RawMessage{}\n\
             \t_ = json.Unmarshal([]byte(body), &object)\n\
             \tfor member, value := range bound {\n\t\tobject[member] = value\n\t}\n\
             \t// Every value in the object is raw JSON already, so re-marshalling\n\
             \t// cannot fail.\n\
             \tfolded, _ := json.Marshal(object)\n\
             \treturn string(folded)\n\
             }",
            vec![import("json", "encoding/json")],
        ),
        (
            "HeaderValue",
            "// HeaderValue reads a response header for a response-bound member as a raw\n\
             // JSON string: nil (encodes as null) when the header is missing. Response\n\
             // header keys are lowercased.\n\
             func HeaderValue(headers map[string]string, name string) json.RawMessage {\n\
             \tvalue, ok := headers[name]\n\
             \tif !ok {\n\t\treturn nil\n\t}\n\
             \t// Marshalling a string cannot fail.\n\
             \tencoded, _ := json.Marshal(value)\n\
             \treturn encoded\n\
             }",
            vec![import("json", "encoding/json")],
        ),
    ])
}
