//! The emitted `internal/transport` package: the struct-request `Send` every
//! generated operation drives, plus the public `support` shapes the canonical
//! transport slot types against. Replaces the descriptor-plus-`Execute()`
//! call into the hand-written HTTP runtime: the generated SDK carries its own
//! transport and imports nothing for it.
//!
//! Poda by use happens at two granularities. The helper functions the clients
//! call by name (`FormatScalar`, `AppendQuery`, ...) are pruned SDK-wide by
//! the usual root-group mechanism. The retry loop and the per-attempt
//! deadline live inside `Send`'s own text and inside `Request`'s own fields,
//! where reachability cannot see them, so those are gated at emission by
//! [`Usage`]: an SDK that never declares `@retry` or `@timeout` gets a
//! transport with none of those pieces in it.

use crate::codegen::symbol::Symbol;
use crate::codegen::tree::Decl;
use crate::ir::{Model, ShapeKind};

use super::{import, support_symbol};

/// Which optional transport pieces any entry in the model actually uses. The
/// emitted `Send`/`Request` text carries only these; an operation that skips a
/// piece its SDK does carry simply leaves the field at its zero value.
#[derive(Clone, Copy, Default)]
pub(crate) struct Usage {
    pub retry: bool,
    pub timeout: bool,
}

impl Usage {
    /// Every piece on: the shape [`super::shared::shared_groups`] declares for
    /// name-to-group resolution, where only which group declares a name
    /// matters, never which pieces this SDK kept.
    pub(crate) fn all() -> Self {
        Usage {
            retry: true,
            timeout: true,
        }
    }
}

/// Read the model's actual usage off the wire bindings.
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
    }
    usage
}

/// One named opaque declaration per row. The table form exists so a run of
/// sibling declarations does not restate the constructor boilerplate row
/// after row; each row is just (name, text, references).
pub(super) fn decl_table(rows: Vec<(&str, &str, Vec<Symbol>)>) -> Vec<Decl> {
    rows.into_iter()
        .map(|(name, text, refs)| Decl::raw_providing(name, text, refs))
        .collect()
}

/// The public, bespoke-facing transport shapes (the SDK's `support` package):
/// the request/response the canonical transport slot (installed directly, or
/// by a generated test) exchanges.
pub(crate) fn support_decls() -> Vec<Decl> {
    decl_table(vec![
        (
            "HTTPRequest",
            "// HTTPRequest is the request the generated transport builds before sending\n\
             // it. Body is nil when the request carries no body.\n\
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
             // case-insensitive).\n\
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
             \t// Success lists the operation's declared success statuses; empty means\n\
             \t// no code was declared, so any 2xx counts instead. Either way, the\n\
             \t// retry loop never retries a response the operation treats as a\n\
             \t// success.\n\
             \tSuccess []int\n\
             \t// Timing is the clock behind the retry backoff; the zero value uses the\n\
             \t// real clock and jitter.\n\
             \tTiming Timing\n",
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
            "// isSuccess: no declared statuses means any 2xx is a success; otherwise\n\
             // only an exact match against the declared statuses counts, even ones\n\
             // inside the 2xx range.\n\
             func isSuccess(status int, declared []int) bool {\n\
             \tif len(declared) == 0 {\n\t\treturn status >= 200 && status < 300\n\t}\n\
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

fn outcome_decl() -> Decl {
    Decl::raw_providing(
        "Outcome",
        "// Outcome is the raw result of one call: a response's status, headers, and\n\
         // body, or the transport failure that prevented one (Cause non-nil). The\n\
         // generated client maps it onto its own error taxonomy; this package stays\n\
         // taxonomy-free so every module can share it.\n\
         type Outcome struct {\n\
         \tStatus  int\n\
         \tHeaders map[string]string\n\
         \tBody    string\n\
         \tCause   error\n\
         }"
        .to_string(),
        Vec::new(),
    )
}

/// One attempt's body: the fresh header copy, the per-attempt deadline, and
/// the dispatch. Shared between the retrying `Send` (where it is the
/// `sendOnce` helper) and the non-retrying one (where it is `Send` whole).
fn attempt_body(u: &Usage) -> String {
    let mut out = String::new();
    out.push_str(
        "\theaders := make(map[string]string, len(req.Headers))\n\
         \tfor name, value := range req.Headers {\n\t\theaders[name] = value\n\t}\n\
         \trequest := support.HTTPRequest{Method: req.Method, URL: req.URL, Headers: headers, Body: req.Body}\n",
    );
    if u.timeout {
        out.push_str(
            "\t// The deadline covers the dispatch (transport call and body read) only.\n",
        );
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
    if !u.retry {
        return vec![Decl::raw_providing(
            "Send",
            format!(
                "// Send performs one operation call: one attempt, dispatched\n\
                 // against the configured transport.\n\
                 func Send{SEND_SIG} {{\n{body}}}",
                body = attempt_body(u),
            ),
            send_refs(),
        )];
    }
    vec![
        Decl::raw_providing(
            "Send",
            format!(
                "// Send performs one operation call: per attempt, one dispatch\n\
                 // against the configured transport.\n\
                 // A retryable outcome (a transport failure, or an error response\n\
                 // Retry.When accepts) repeats up to Retry.Max times, with exponential\n\
                 // full-jitter backoff between attempts.\n\
                 func Send{SEND_SIG} {{\n\
                 \tfor attempt := 0; ; attempt++ {{\n\
                 \t\toutcome := sendOnce(ctx, native, canonical, req)\n\
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

/// The whole `internal/transport` package, shaped by what the SDK uses.
pub(crate) fn internal_helpers(u: &Usage) -> Vec<Decl> {
    let mut decls = vec![request_decl(u)];
    if u.retry {
        decls.extend(retry_decls());
    }
    decls.push(outcome_decl());
    decls.extend(send_decls(u));
    decls.push(super::assembly::dispatch_decl());
    decls.extend(super::assembly::assembly_decls());
    decls
}

#[cfg(test)]
#[path = "send_tests.rs"]
mod tests;
