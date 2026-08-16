//! The request-assembly half of the emitted `internal/transport` package:
//! the helpers the generated call sites name to build a URL, layer headers,
//! and encode a body, plus the raw dispatch behind one attempt. Split from
//! [`super::send`] (which owns the call policy: Send, retry, timeout) along
//! that seam, so each half stays readable on its own.

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
            "// AppendQuery serializes a query value as a repeated entry per element for\n\
             // a list, a single entry otherwise; a nil (absent) value is omitted, the\n\
             // body's nullable-omit rule applied to the request line.\n\
             func AppendQuery(entries []string, name string, value any) []string {\n\
             \tif value == nil {\n\t\treturn entries\n\t}\n\
             \tadd := func(v any) []string {\n\
             \t\treturn append(entries, url.QueryEscape(name)+\"=\"+url.QueryEscape(FormatScalar(v)))\n\
             \t}\n\
             \tif list, ok := value.([]any); ok {\n\
             \t\tfor _, element := range list {\n\t\t\tentries = add(element)\n\t\t}\n\
             \t\treturn entries\n\
             \t}\n\
             \treturn add(value)\n\
             }",
            vec![import("url", "net/url")],
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
            "// EncodeBody assembles the body-bound members into a JSON object, in the\n\
             // given member order. A member that is present but null still lands in the\n\
             // object as null; only absence omits it. Nil when no member is present.\n\
             func EncodeBody(record map[string]any, members ...string) ([]byte, error) {\n\
             \tvar fields []byte\n\
             \tfor _, member := range members {\n\
             \t\tv, ok := record[member]\n\
             \t\tif !ok {\n\t\t\tcontinue\n\t\t}\n\
             \t\t// Marshalling a string cannot fail, so the key carries no error path.\n\
             \t\tkey, _ := json.Marshal(member)\n\
             \t\tvalue, err := json.Marshal(v)\n\
             \t\tif err != nil {\n\t\t\treturn nil, err\n\t\t}\n\
             \t\tif fields == nil {\n\t\t\tfields = append(fields, '{')\n\t\t} else {\n\t\t\tfields = append(fields, ',')\n\t\t}\n\
             \t\tfields = append(fields, key...)\n\
             \t\tfields = append(fields, ':')\n\
             \t\tfields = append(fields, value...)\n\
             \t}\n\
             \tif fields == nil {\n\t\treturn nil, nil\n\t}\n\
             \treturn append(fields, '}'), nil\n\
             }",
            vec![import("json", "encoding/json")],
        ),
        (
            "FoldResponse",
            "// FoldResponse folds the response-bound members (a header value, or the\n\
             // status code) into the decoded body so the generated decoder sees them as\n\
             // ordinary fields. Applied on the success path only; a non-object or empty\n\
             // body leaves the bound fields to stand on their own.\n\
             func FoldResponse(body string, bound map[string]any) string {\n\
             \tobject := map[string]any{}\n\
             \t_ = json.Unmarshal([]byte(body), &object)\n\
             \tfor member, value := range bound {\n\t\tobject[member] = value\n\t}\n\
             \t// Every value came off decoded JSON, an int status, or a header string,\n\
             \t// so re-marshalling cannot fail.\n\
             \tfolded, _ := json.Marshal(object)\n\
             \treturn string(folded)\n\
             }",
            vec![import("json", "encoding/json")],
        ),
        (
            "HeaderValue",
            "// HeaderValue reads a response header for a response-bound member: nil\n\
             // (the member decodes as absent) when the header is missing. Response\n\
             // header keys are lowercased.\n\
             func HeaderValue(headers map[string]string, name string) any {\n\
             \tif value, ok := headers[name]; ok {\n\t\treturn value\n\t}\n\
             \treturn nil\n\
             }",
            Vec::new(),
        ),
    ])
}
