//! The emitted `internal/transport` package: the struct-request `Send` every
//! generated operation drives, plus the public `support` shapes a bespoke hook
//! types against. Replaces the descriptor-plus-`Execute()` call into the
//! hand-written HTTP runtime: the generated SDK carries its own transport and
//! imports nothing for it.
//!
//! Poda by use happens at two granularities. The helper functions the clients
//! call by name (`FormatScalar`, `AppendQuery`, ...) are pruned SDK-wide by
//! the usual root-group mechanism. The retry loop, the per-attempt deadline,
//! and the hook slots live inside `Send`'s own text and inside `Request`'s
//! own fields, where reachability cannot see them, so those are gated at
//! emission by [`Usage`]: an SDK that never declares `@retry`, `@timeout`, or
//! a request hook gets a transport with none of those pieces in it.

use crate::codegen::extensions::hook_binding;
use crate::codegen::symbol::Symbol;
use crate::codegen::tree::Decl;
use crate::ir::{Model, ShapeKind};

use super::{import, support_symbol, BINDING_LANGS};

/// Which optional transport pieces any entry in the model actually uses. The
/// emitted `Send`/`Request` text carries only these; an operation that skips a
/// piece its SDK does carry simply leaves the field at its zero value.
#[derive(Clone, Copy, Default)]
pub(crate) struct Usage {
    pub retry: bool,
    pub timeout: bool,
    pub hooks: bool,
}

impl Usage {
    /// Every piece on: the shape [`super::shared::shared_groups`] declares for
    /// name-to-group resolution, where only which group declares a name
    /// matters, never which pieces this SDK kept.
    pub(crate) fn all() -> Self {
        Usage {
            retry: true,
            timeout: true,
            hooks: true,
        }
    }
}

/// Read the model's actual usage off the wire bindings and the bound hooks.
pub(crate) fn usage_of(model: &Model) -> Usage {
    let mut usage = Usage::default();
    for module in &model.modules {
        for shape in &module.shapes {
            let ShapeKind::Entry { operations, .. } = &shape.kind else {
                continue;
            };
            for op in operations {
                if let ShapeKind::Operation { wire: Some(w), .. } = &op.kind {
                    usage.retry |= w.retry.is_some();
                    usage.timeout |= w.timeout.is_some();
                }
            }
        }
        if let Some((_, _, bound)) =
            crate::codegen::entries::plan::entry_setup(module, &BINDING_LANGS)
        {
            usage.hooks |= hook_binding(&bound, "before_request").is_some()
                || hook_binding(&bound, "after_response").is_some();
        }
    }
    usage
}

/// One named opaque declaration per row. The table form exists so a run of
/// sibling declarations does not restate the constructor boilerplate row
/// after row; each row is just (name, text, references).
fn decl_table(rows: Vec<(&str, &str, Vec<Symbol>)>) -> Vec<Decl> {
    rows.into_iter()
        .map(|(name, text, refs)| Decl::raw_providing(name, text, refs))
        .collect()
}

/// The public, bespoke-facing transport shapes (the SDK's `support` package):
/// the request/response a bound `before_request`/`after_response` hook types
/// against, and the canonical transport slot a `client_init` hook (or a
/// generated test) installs.
pub(crate) fn support_decls() -> Vec<Decl> {
    decl_table(vec![
        (
            "HTTPRequest",
            "// HTTPRequest is the request the generated transport builds before sending\n\
             // it. A before_request hook receives it and may return a mutated copy (set\n\
             // an auth header, sign the body). Body is nil when the request carries no\n\
             // body.\n\
             type HTTPRequest struct {\n\
             \tMethod  string\n\
             \tURL     string\n\
             \tHeaders map[string]string\n\
             \tBody    []byte\n\
             }",
            Vec::new(),
        ),
        (
            "HTTPResponse",
            "// HTTPResponse is the response the generated transport reads before\n\
             // classifying it. Header keys are lowercased (HTTP header names are\n\
             // case-insensitive). An after_response hook may return a mutated copy.\n\
             type HTTPResponse struct {\n\
             \tStatus  int\n\
             \tHeaders map[string]string\n\
             \tBody    string\n\
             }",
            Vec::new(),
        ),
        (
            "HTTPTransport",
            "// HTTPTransport is the canonical transport slot: adapt any HTTP stack by\n\
             // mapping HTTPRequest/HTTPResponse, without emulating *http.Client. One\n\
             // call is one attempt: the generated client owns retry, so a transport\n\
             // with internal retries does not combine with it. The transport must honor\n\
             // ctx cancellation; a declared per-attempt timeout arrives as a ctx\n\
             // deadline.\n\
             type HTTPTransport func(ctx context.Context, req HTTPRequest) (HTTPResponse, error)",
            vec![
                import("context", "context"),
                support_symbol("HTTPRequest"),
                support_symbol("HTTPResponse"),
            ],
        ),
    ])
}

/// The `Request` struct: always the assembled call itself; the policy fields
/// ride only when something in the SDK declares the policy.
fn request_decl(u: &Usage) -> Decl {
    let mut fields = String::from(
        "\tMethod  string\n\
         \tURL     string\n\
         \tHeaders map[string]string\n\
         \t// Body is the encoded request body; nil sends none.\n\
         \tBody []byte\n",
    );
    let mut refs = Vec::new();
    if u.timeout {
        fields.push_str(
            "\t// Timeout bounds each attempt separately, as a context deadline around\n\
             \t// the dispatch; zero means no deadline.\n\
             \tTimeout time.Duration\n",
        );
        refs.push(import("time", "time"));
    }
    if u.retry {
        fields.push_str(
            "\t// Retry is the operation's declared retry policy; the zero value never\n\
             \t// retries.\n\
             \tRetry Retry\n\
             \t// Success lists the operation's declared success statuses outside the\n\
             \t// 2xx range (any 2xx always counts), so the retry loop never retries a\n\
             \t// response the operation treats as a success.\n\
             \tSuccess []int\n\
             \t// Timing is the clock behind the retry backoff; the zero value uses the\n\
             \t// real clock and jitter.\n\
             \tTiming Timing\n",
        );
    }
    if u.hooks {
        fields.push_str(
            "\t// Hooks are the bound lifecycle slots, invoked around every attempt;\n\
             \t// nil skips them.\n\
             \tHooks *Hooks\n",
        );
    }
    Decl::raw_providing(
        "Request",
        format!(
            "// Request is one operation's HTTP call as the generated client assembled\n\
             // it: the finished URL, the layered headers, and the encoded body, plus\n\
             // the operation's declared call policy.\n\
             type Request struct {{\n{fields}}}"
        ),
        refs,
    )
}

fn retry_decls() -> Vec<Decl> {
    decl_table(vec![
        (
            "Retry",
            "// Retry is an operation's declared retry policy: Max retries beyond the\n\
             // first attempt, and When classifying an error response (status and raw\n\
             // body) as retryable. A transport failure always retries while attempts\n\
             // remain; a nil When means no error response ever does.\n\
             type Retry struct {\n\
             \tMax  int\n\
             \tWhen func(status int, body string) bool\n\
             }",
            Vec::new(),
        ),
        (
            "Timing",
            "// Timing is the clock behind the retry backoff, swappable so a test can\n\
             // pin the jitter and record the sleeps instead of waiting them out. The\n\
             // generated client holds one in an unexported field, which is what keeps\n\
             // the seam reachable from its own package's tests and from nowhere else.\n\
             type Timing struct {\n\
             \tSleep  func(ctx context.Context, d time.Duration) error\n\
             \tRandom func() float64\n\
             }",
            vec![import("context", "context"), import("time", "time")],
        ),
        (
            "retryable",
            "// retryable classifies one outcome for the retry loop: a transport failure\n\
             // always retries; a success never does; an error response only when\n\
             // Retry.When accepts its status and raw body.\n\
             func retryable(o Outcome, req Request) bool {\n\
             \tif o.Cause != nil {\n\t\treturn true\n\t}\n\
             \tif isSuccess(o.Status, req.Success) || req.Retry.When == nil {\n\t\treturn false\n\t}\n\
             \treturn req.Retry.When(o.Status, o.Body)\n\
             }",
            Vec::new(),
        ),
        (
            "isSuccess",
            "// isSuccess: any 2xx is a success even when its exact code was not the one\n\
             // declared (a server may answer 200 where 201 was declared); a declared\n\
             // success code outside the 2xx range still counts.\n\
             func isSuccess(status int, declared []int) bool {\n\
             \tif status >= 200 && status < 300 {\n\t\treturn true\n\t}\n\
             \tfor _, s := range declared {\n\t\tif s == status {\n\t\t\treturn true\n\t\t}\n\t}\n\
             \treturn false\n\
             }",
            Vec::new(),
        ),
        (
            "backoffDelay",
            "// backoffDelay is exponential with full jitter: random * min(2000ms, 100ms\n\
             // * 2^attempt). The constants are part of the cross-runtime parity\n\
             // contract and must match every other target.\n\
             func backoffDelay(attempt int, random float64) time.Duration {\n\
             \texp := math.Min(2000, 100*math.Pow(2, float64(attempt)))\n\
             \treturn time.Duration(random * exp * float64(time.Millisecond))\n\
             }",
            vec![import("math", "math"), import("time", "time")],
        ),
        (
            "retryDelay",
            "// retryDelay waits out one attempt's backoff, on the request's timing seam\n\
             // when a test pinned it and the real clock otherwise. Jitter seeds only\n\
             // this backoff, never anything security-sensitive, so the default PRNG is\n\
             // fine.\n\
             func retryDelay(ctx context.Context, attempt int, timing Timing) error {\n\
             \trandom := timing.Random\n\
             \tif random == nil {\n\t\trandom = rand.Float64\n\t}\n\
             \tsleep := timing.Sleep\n\
             \tif sleep == nil {\n\t\tsleep = sleepContext\n\t}\n\
             \treturn sleep(ctx, backoffDelay(attempt, random()))\n\
             }",
            vec![import("context", "context"), import("rand", "math/rand")],
        ),
        (
            "sleepContext",
            "func sleepContext(ctx context.Context, d time.Duration) error {\n\
             \ttimer := time.NewTimer(d)\n\
             \tdefer timer.Stop()\n\
             \tselect {\n\
             \tcase <-ctx.Done():\n\t\treturn ctx.Err()\n\
             \tcase <-timer.C:\n\t\treturn nil\n\
             \t}\n\
             }",
            vec![import("context", "context"), import("time", "time")],
        ),
    ])
}

fn hooks_decl() -> Decl {
    Decl::raw_providing(
        "Hooks",
        "// Hooks are the lifecycle slots the generated client binds around the\n\
         // transport, invoked once per attempt. A hook error propagates raw: it is\n\
         // never misreported as a transport failure and is never retried.\n\
         type Hooks struct {\n\
         \tBeforeRequest func(ctx context.Context, req support.HTTPRequest) (support.HTTPRequest, error)\n\
         \tAfterResponse func(ctx context.Context, res support.HTTPResponse) (support.HTTPResponse, error)\n\
         }"
        .to_string(),
        vec![
            import("context", "context"),
            support_symbol("HTTPRequest"),
            support_symbol("HTTPResponse"),
        ],
    )
}

fn outcome_decl(u: &Usage) -> Decl {
    let hook_err = if u.hooks {
        "\t// HookErr is a lifecycle hook's own failure, carried raw: the generated\n\
         \t// client propagates it as-is, and it is never misreported as a transport\n\
         \t// failure and never retried.\n\
         \tHookErr error\n"
    } else {
        ""
    };
    Decl::raw_providing(
        "Outcome",
        format!(
            "// Outcome is the raw result of one call: a response's status, headers, and\n\
             // body, or the transport failure that prevented one (Cause non-nil). The\n\
             // generated client maps it onto its own error taxonomy; this package stays\n\
             // taxonomy-free so every module can share it.\n\
             type Outcome struct {{\n\
             \tStatus  int\n\
             \tHeaders map[string]string\n\
             \tBody    string\n\
             \tCause   error\n\
             {hook_err}}}"
        ),
        Vec::new(),
    )
}

/// One attempt's body: the fresh header copy, the hook slots, the per-attempt
/// deadline, and the dispatch. Shared between the retrying `Send` (where it is
/// the `sendOnce` helper) and the non-retrying one (where it is `Send` whole).
fn attempt_body(u: &Usage) -> String {
    let mut out = String::new();
    // A fresh copy per attempt: a hook may mutate the headers it receives in
    // place, and a retried attempt must not inherit that.
    out.push_str(
        "\theaders := make(map[string]string, len(req.Headers))\n\
         \tfor name, value := range req.Headers {\n\t\theaders[name] = value\n\t}\n\
         \trequest := support.HTTPRequest{Method: req.Method, URL: req.URL, Headers: headers, Body: req.Body}\n",
    );
    if u.hooks {
        out.push_str(
            "\tif req.Hooks != nil && req.Hooks.BeforeRequest != nil {\n\
             \t\thooked, err := req.Hooks.BeforeRequest(ctx, request)\n\
             \t\tif err != nil {\n\t\t\treturn Outcome{HookErr: err}\n\t\t}\n\
             \t\trequest = hooked\n\
             \t}\n",
        );
    }
    if u.timeout {
        let why = if u.hooks {
            "\t// The deadline covers the dispatch (transport call and body read) only,\n\
             \t// starting after the BeforeRequest hook so a slow hook does not eat into\n\
             \t// the attempt.\n"
        } else {
            "\t// The deadline covers the dispatch (transport call and body read) only.\n"
        };
        out.push_str(why);
        out.push_str(
            "\tattemptCtx := ctx\n\
             \tif req.Timeout > 0 {\n\
             \t\tvar cancel context.CancelFunc\n\
             \t\tattemptCtx, cancel = context.WithTimeout(ctx, req.Timeout)\n\
             \t\tdefer cancel()\n\
             \t}\n\
             \tresponse, err := dispatch(attemptCtx, native, canonical, request)\n",
        );
    } else {
        out.push_str("\tresponse, err := dispatch(ctx, native, canonical, request)\n");
    }
    out.push_str("\tif err != nil {\n\t\treturn Outcome{Cause: err}\n\t}\n");
    if u.hooks {
        out.push_str(
            "\tif req.Hooks != nil && req.Hooks.AfterResponse != nil {\n\
             \t\thooked, err := req.Hooks.AfterResponse(ctx, response)\n\
             \t\tif err != nil {\n\t\t\treturn Outcome{HookErr: err}\n\t\t}\n\
             \t\tresponse = hooked\n\
             \t}\n",
        );
    }
    out.push_str(
        "\treturn Outcome{Status: response.Status, Headers: response.Headers, Body: response.Body}\n",
    );
    out
}

const SEND_SIG: &str =
    "(ctx context.Context, native *http.Client, canonical support.HTTPTransport, req Request) Outcome";

fn send_refs() -> Vec<Symbol> {
    vec![
        import("context", "context"),
        import("http", "net/http"),
        support_symbol("HTTPTransport"),
        support_symbol("HTTPRequest"),
    ]
}

/// `Send`: the whole call. With retry in the SDK it is the loop around
/// `sendOnce`; without, it is the single attempt itself, under the same name
/// and signature either way.
fn send_decls(u: &Usage) -> Vec<Decl> {
    let hook_doc = if u.hooks {
        "// around the hook slots. A hook's own failure comes back as\n\
         // Outcome.HookErr, raw: never retried, never misreported as a transport\n\
         // failure.\n"
    } else {
        "// against the configured transport.\n"
    };
    if !u.retry {
        return vec![Decl::raw_providing(
            "Send",
            format!(
                "// Send performs one operation call: one attempt, dispatched\n\
                 {hook_doc}\
                 func Send{SEND_SIG} {{\n{body}}}",
                body = attempt_body(u),
            ),
            send_refs(),
        )];
    }
    let hook_bail = if u.hooks {
        "\t\tif outcome.HookErr != nil {\n\t\t\treturn outcome\n\t\t}\n"
    } else {
        ""
    };
    vec![
        Decl::raw_providing(
            "Send",
            format!(
                "// Send performs one operation call: per attempt, one dispatch\n\
                 {hook_doc}\
                 // A retryable outcome (a transport failure, or an error response\n\
                 // Retry.When accepts) repeats up to Retry.Max times, with exponential\n\
                 // full-jitter backoff between attempts.\n\
                 func Send{SEND_SIG} {{\n\
                 \tfor attempt := 0; ; attempt++ {{\n\
                 \t\toutcome := sendOnce(ctx, native, canonical, req)\n\
                 {hook_bail}\
                 \t\tif attempt >= req.Retry.Max || !retryable(outcome, req) {{\n\t\t\treturn outcome\n\t\t}}\n\
                 \t\tif err := retryDelay(ctx, attempt, req.Timing); err != nil {{\n\
                 \t\t\t// The caller gave up while we were waiting to retry: surface the\n\
                 \t\t\t// cancellation, not the stale outcome it interrupts.\n\
                 \t\t\treturn Outcome{{Cause: err}}\n\
                 \t\t}}\n\
                 \t}}\n\
                 }}"
            ),
            send_refs(),
        ),
        Decl::raw_providing(
            "sendOnce",
            format!(
                "// sendOnce is one attempt of [Send].\n\
                 func sendOnce{SEND_SIG} {{\n{body}}}",
                body = attempt_body(u),
            ),
            send_refs(),
        ),
    ]
}

fn dispatch_decl() -> Decl {
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
fn assembly_decls() -> Vec<Decl> {
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

/// The whole `internal/transport` package, shaped by what the SDK uses.
pub(crate) fn internal_helpers(u: &Usage) -> Vec<Decl> {
    let mut decls = vec![request_decl(u)];
    if u.retry {
        decls.extend(retry_decls());
    }
    if u.hooks {
        decls.push(hooks_decl());
    }
    decls.push(outcome_decl(u));
    decls.extend(send_decls(u));
    decls.push(dispatch_decl());
    decls.extend(assembly_decls());
    decls
}

#[cfg(test)]
#[path = "send_tests.rs"]
mod tests;
