//! The shared support types and internal helpers every emitted TypeScript
//! transport call draws on, split from `transport.rs` to stay within the
//! file-size gate. Poda by use is the usual root-group mechanism: an SDK
//! with no `@retry` anywhere drops the backoff helpers entirely.

use crate::codegen::tree::Decl;

use super::support_symbol;

/// The public, bespoke-facing transport types (`Group::root_support()`): the
/// request/response shapes a bound `before_request`/`after_response` hook
/// type-checks against, plus the construction-time `ClientOptions`/`Hooks`.
pub(crate) fn http_support_decls() -> Vec<Decl> {
    vec![
        Decl::raw_providing(
            "HttpResponse",
            "// HttpResponse is the response the runtime reads before classifying it.\n\
             // An after_response hook may return a mutated copy.\n\
             export interface HttpResponse {\n  status: number;\n  headers: Record<string, string>;\n  body: string;\n}",
            Vec::new(),
        ),
        Decl::raw_providing(
            "HttpRequest",
            "// HttpRequest is the request the runtime builds before sending it. A\n\
             // before_request hook receives this and may return a mutated copy (set an\n\
             // auth header, sign the body).\n\
             export interface HttpRequest {\n  method: string;\n  url: string;\n  headers: Record<string, string>;\n  body: string | undefined;\n}",
            Vec::new(),
        ),
        Decl::raw_providing(
            "HttpTransport",
            "// HttpTransport adapts any HTTP stack by mapping HttpRequest/HttpResponse,\n\
             // without emulating fetch. One call is one attempt: the generated client\n\
             // owns retry, so a transport with internal retries does not combine with\n\
             // it.\n\
             export type HttpTransport = (req: HttpRequest, signal?: AbortSignal) => Promise<HttpResponse>;",
            vec![support_symbol("HttpRequest"), support_symbol("HttpResponse")],
        ),
        Decl::raw_providing(
            "ClientOptions",
            "// ClientOptions is what the caller supplies once, shared across every\n\
             // operation. Exactly one transport slot may be set: fetch (native) or\n\
             // transport (canonical); setting both is a construction error. No slot\n\
             // ships its own auth; a bespoke hook sets an auth header through headers.\n\
             export interface ClientOptions {\n  readonly fetch?: typeof fetch;\n  readonly transport?: HttpTransport;\n  readonly headers?: Readonly<Record<string, string>>;\n}",
            vec![support_symbol("HttpTransport")],
        ),
        Decl::raw_providing(
            "Hooks",
            "// Hooks are the lifecycle slots the generated client invokes around the\n\
             // transport. A hook that throws propagates raw; the generated wrapper is\n\
             // what turns it into a ContractError.\n\
             export interface Hooks {\n  readonly before_request?: (req: HttpRequest) => HttpRequest | Promise<HttpRequest>;\n  readonly after_response?: (res: HttpResponse) => HttpResponse | Promise<HttpResponse>;\n}",
            vec![support_symbol("HttpRequest"), support_symbol("HttpResponse")],
        ),
    ]
}

/// One HTTP dispatch function: `httpSend` and `httpSendWithTimeout` share the
/// same leading `(options: ClientOptions, request: HttpRequest, ...)` shape
/// and the same three-type refs list, so both are built through this one
/// place rather than as two near-identical `Decl::raw_providing` calls.
fn http_dispatch_fn(name: &str, doc: &str, extra_params: &str, body: &str) -> Decl {
    Decl::raw_providing(
        name,
        format!(
            "{doc}\n\
             export async function {name}(\n\
             \x20 options: ClientOptions,\n\
             \x20 request: HttpRequest,\n\
             {extra_params}\
             ): Promise<HttpResponse> {{\n\
             {body}\n\
             }}"
        ),
        vec![
            support_symbol("ClientOptions"),
            support_symbol("HttpRequest"),
            support_symbol("HttpResponse"),
        ],
    )
}

/// The internal transport helpers (`Group::root("http")`), pruned SDK-wide by
/// usage exactly like the `duration`/`casing` groups: an SDK with no
/// `@retry` anywhere never carries the backoff helpers.
pub(crate) fn internal_helpers() -> Vec<Decl> {
    vec![
        Decl::raw_providing(
            "formatScalar",
            "// formatScalar renders a value the way the wire expects it in a path,\n\
             // query, or header position: strings verbatim, everything else as JSON.\n\
             export function formatScalar(value: unknown): string {\n\
             \x20 if (typeof value === \"string\") return value;\n\
             \x20 try {\n\
             \x20   return JSON.stringify(value) ?? \"\";\n\
             \x20 } catch {\n\
             \x20   return \"\";\n\
             \x20 }\n\
             }",
            Vec::new(),
        ),
        Decl::raw_providing(
            "pathPart",
            "// pathPart renders a path segment: an absent value substitutes empty\n\
             // rather than the literal \"undefined\"/\"null\".\n\
             export function pathPart(value: unknown): string {\n\
             \x20 return value === undefined || value === null ? \"\" : encodeURIComponent(formatScalar(value));\n\
             }",
            Vec::new(),
        ),
        Decl::raw_providing(
            "setHeader",
            "// setHeader overrides across casings: header names are case-insensitive,\n\
             // so a bespoke \"authorization\" replaces a declared \"Authorization\" rather\n\
             // than riding beside it.\n\
             export function setHeader(headers: Record<string, string>, name: string, value: string): void {\n\
             \x20 const lower = name.toLowerCase();\n\
             \x20 for (const key of Object.keys(headers)) {\n\
             \x20   if (key.toLowerCase() === lower) delete headers[key];\n\
             \x20 }\n\
             \x20 headers[name] = value;\n\
             }",
            Vec::new(),
        ),
        Decl::raw_providing(
            "hasHeader",
            "export function hasHeader(headers: Record<string, string>, name: string): boolean {\n\
             \x20 const lower = name.toLowerCase();\n\
             \x20 return Object.keys(headers).some((key) => key.toLowerCase() === lower);\n\
             }",
            Vec::new(),
        ),
        Decl::raw_providing(
            "headerRecord",
            "// headerRecord collects a Headers object into a plain record so a hook can\n\
             // read and rewrite it without a live Headers instance.\n\
             export function headerRecord(headers: Headers): Record<string, string> {\n\
             \x20 const record: Record<string, string> = {};\n\
             \x20 headers.forEach((value, key) => {\n\
             \x20   record[key] = value;\n\
             \x20 });\n\
             \x20 return record;\n\
             }",
            Vec::new(),
        ),
        Decl::raw_providing(
            "appendQuery",
            "// appendQuery serializes as a repeated entry per element for a list, a\n\
             // single entry otherwise; a null/absent value is omitted.\n\
             export function appendQuery(qs: URLSearchParams, name: string, value: unknown): void {\n\
             \x20 if (value === undefined || value === null) return;\n\
             \x20 if (Array.isArray(value)) {\n\
             \x20   for (const element of value) qs.append(name, String(element));\n\
             \x20 } else {\n\
             \x20   qs.append(name, String(value));\n\
             \x20 }\n\
             }",
            Vec::new(),
        ),
        Decl::raw_providing(
            "parseJsonObject",
            "// parseJsonObject parses a response body for response-bound member\n\
             // folding; a non-object or unparsable body leaves the bound fields to\n\
             // stand on their own.\n\
             export function parseJsonObject(body: string): Record<string, unknown> {\n\
             \x20 if (body === \"\") return {};\n\
             \x20 try {\n\
             \x20   const parsed: unknown = JSON.parse(body);\n\
             \x20   return parsed !== null && typeof parsed === \"object\" ? (parsed as Record<string, unknown>) : {};\n\
             \x20 } catch {\n\
             \x20   return {};\n\
             \x20 }\n\
             }",
            Vec::new(),
        ),
        Decl::raw_providing(
            "assertExclusiveTransport",
            "// assertExclusiveTransport rejects setting both transport slots: the\n\
             // caller must pick the native slot (fetch) or the canonical one\n\
             // (transport), not both.\n\
             export function assertExclusiveTransport(options: ClientOptions): void {\n\
             \x20 if (options.fetch && options.transport) {\n\
             \x20   throw new Error(\n\
             \x20     \"ClientOptions.fetch and ClientOptions.transport are mutually exclusive: set the native slot or the canonical slot, not both\",\n\
             \x20   );\n\
             \x20 }\n\
             }",
            vec![support_symbol("ClientOptions")],
        ),
        http_dispatch_fn(
            "httpSend",
            "// httpSend performs one attempt: the canonical transport when set,\n\
             // otherwise fetch.",
            "\x20 signal: AbortSignal | undefined,\n",
            "\x20 if (options.transport) return options.transport(request, signal);\n\
             \x20 const transport = options.fetch ?? fetch;\n\
             \x20 const response = await transport(request.url, {\n\
             \x20   method: request.method,\n\
             \x20   headers: request.headers,\n\
             \x20   body: request.body,\n\
             \x20   signal,\n\
             \x20 });\n\
             \x20 const text = await response.text();\n\
             \x20 return { status: response.status, headers: headerRecord(response.headers), body: text };",
        ),
        http_dispatch_fn(
            "httpSendWithTimeout",
            "// httpSendWithTimeout bounds one attempt: the timeout aborts the signal\n\
             // (so a cooperating transport cancels its work) and rejects the attempt\n\
             // regardless, so a transport that ignores the signal still times out.",
            "\x20 timeoutMs: number,\n",
            "\x20 if (timeoutMs <= 0) return httpSend(options, request, undefined);\n\
             \x20 const controller = new AbortController();\n\
             \x20 let timer: ReturnType<typeof setTimeout> | undefined;\n\
             \x20 const expiry = new Promise<never>((_, reject) => {\n\
             \x20   timer = setTimeout(() => {\n\
             \x20     const cause = new Error(`attempt timed out after ${timeoutMs}ms`);\n\
             \x20     controller.abort(cause);\n\
             \x20     reject(cause);\n\
             \x20   }, timeoutMs);\n\
             \x20 });\n\
             \x20 const call = httpSend(options, request, controller.signal);\n\
             \x20 try {\n\
             \x20   const raced = Promise.race([call, expiry]);\n\
             \x20   call.catch(() => {});\n\
             \x20   return await raced;\n\
             \x20 } finally {\n\
             \x20   clearTimeout(timer);\n\
             \x20 }",
        ),
        Decl::raw_providing(
            "backoffDelayMs",
            "// backoffDelayMs is exponential with full jitter: the constants are part\n\
             // of the cross-runtime parity contract and must match every other target.\n\
             export function backoffDelayMs(attempt: number, random: number): number {\n\
             \x20 return random * Math.min(2000, 100 * 2 ** attempt);\n\
             }",
            Vec::new(),
        ),
        Decl::raw_providing(
            "resolveMaxRetries",
            "// resolveMaxRetries clamps the operation's @retry field: a non-finite\n\
             // value or one below one both mean zero retries; a fractional value\n\
             // floors. bigint (an i64/u64-typed @retry field) narrows to a number\n\
             // first, the same conversion every other numeric ref narrows through.\n\
             export function resolveMaxRetries(value: number | bigint): number {\n\
             \x20 const n = typeof value === \"bigint\" ? Number(value) : value;\n\
             \x20 return Number.isFinite(n) && n >= 1 ? Math.floor(n) : 0;\n\
             }",
            Vec::new(),
        ),
        Decl::raw_providing(
            "timingSeam",
            "// timingSeam is the sleep/random behind the retry loop's backoff, as a\n\
             // mutable object rather than fixed functions: an ES module import\n\
             // binding is read-only, so a plain `export function` could never be\n\
             // substituted from outside its own file, but a property of an\n\
             // exported object can. Internal to the package (this group is\n\
             // excluded from package.json's exports map), so only code shipped in\n\
             // the same SDK, never a consumer, can reach or override it. Math.random\n\
             // seeds only this jitter, never anything security-sensitive (no token,\n\
             // no session id, no cryptographic use), so a predictable PRNG default\n\
             // is fine.\n\
             export const timingSeam: { sleep: (ms: number) => Promise<void>; random: () => number } = {\n\
             \x20 sleep: (ms) => new Promise((resolve) => setTimeout(resolve, ms)),\n\
             \x20 random: () => Math.random(), // NOSONAR: jitter timing only, not a cryptographic use\n\
             };",
            Vec::new(),
        ),
        Decl::raw_providing(
            "retryDelay",
            "// retryDelay waits out one attempt's exponential-backoff delay before a\n\
             // retried call.\n\
             export async function retryDelay(attempt: number): Promise<void> {\n\
             \x20 await timingSeam.sleep(backoffDelayMs(attempt, timingSeam.random()));\n\
             }",
            Vec::new(),
        ),
    ]
}
