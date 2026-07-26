package tonohttp

import (
	"bytes"
	"context"
	"encoding/json"
	"io"
	"net/http"
	"strings"
	"time"
)

// Execute interprets one wire descriptor against the configured transport and
// returns the raw Outcome. The returned error is reserved for a hook failure
// or an unencodable input: those propagate raw for the generated wrapper to
// classify, and are never misreported as transport failures nor retried.
//
// When the descriptor declares retry, an attempt whose outcome is retryable (a
// transport failure, or an error response matching a declared retryable error)
// is repeated up to the declared maximum, with exponential full-jitter backoff
// between attempts. The declared timeout bounds each attempt separately.
func (r *Runtime) Execute(ctx context.Context, d *WireDescriptor, input map[string]any, hooks *Hooks) (Outcome, error) {
	maxRetries := resolveMaxRetries(d.Retry, r.opts.Values)
	timeout := resolveTimeout(d.Timeout, r.opts.Values)
	for attempt := 0; ; attempt++ {
		outcome, err := r.attempt(ctx, d, input, hooks, timeout)
		if err != nil {
			return Outcome{}, err
		}
		if attempt >= maxRetries || !isRetryable(d, outcome) {
			return outcome, nil
		}
		if err := r.sleep(ctx, backoffDelay(attempt, r.randFloat())); err != nil {
			// The caller gave up while we were waiting to retry: surface the
			// cancellation, not the stale outcome it interrupts.
			return Outcome{Kind: OutcomeTransport, Cause: err}, nil
		}
	}
}

func (r *Runtime) attempt(ctx context.Context, d *WireDescriptor, input map[string]any, hooks *Hooks, timeout time.Duration) (Outcome, error) {
	body, err := buildBody(d, input)
	if err != nil {
		return Outcome{}, err
	}
	headers := buildHeaders(d, input, r.opts.Headers, r.opts.Values)
	if body != nil && !hasHeader(headers, "content-type") {
		headers["content-type"] = "application/json"
	}
	base := r.opts.BaseURL
	if endpoint, ok := resolveEndpoint(d.Endpoint, r.opts.Values); ok {
		base = endpoint
	}
	requestURL := base + buildPath(d, input, r.opts.Values)
	if qs := buildQuery(d, input); qs != "" {
		requestURL += "?" + qs
	}

	request := CanonicalRequest{Method: d.HTTPMethod, URL: requestURL, Headers: headers, Body: body}
	if hooks != nil && hooks.BeforeRequest != nil {
		request, err = hooks.BeforeRequest(ctx, request)
		if err != nil {
			return Outcome{}, err
		}
	}

	// The timeout covers the transport call and body read only, starting after
	// the BeforeRequest hook so a slow hook does not eat into the attempt.
	attemptCtx := ctx
	if timeout > 0 {
		var cancel context.CancelFunc
		attemptCtx, cancel = context.WithTimeout(ctx, timeout)
		defer cancel()
	}
	response, transportErr := r.callTransport(attemptCtx, request)
	if transportErr != nil {
		return Outcome{Kind: OutcomeTransport, Cause: transportErr}, nil
	}

	if hooks != nil && hooks.AfterResponse != nil {
		response, err = hooks.AfterResponse(ctx, response)
		if err != nil {
			return Outcome{}, err
		}
	}

	if isSuccessStatus(d, response.Status) {
		return Outcome{Kind: OutcomeSuccess, Status: response.Status, Body: applyResponseBindings(d, response)}, nil
	}
	return Outcome{Kind: OutcomeError, Status: response.Status, Body: response.Body}, nil
}

func (r *Runtime) callTransport(ctx context.Context, req CanonicalRequest) (CanonicalResponse, error) {
	if r.opts.Transport != nil {
		return r.opts.Transport(ctx, req)
	}
	client := r.opts.Client
	if client == nil {
		client = http.DefaultClient
	}
	var reader io.Reader
	if req.Body != nil {
		reader = bytes.NewReader(req.Body)
	}
	httpReq, err := http.NewRequestWithContext(ctx, req.Method, req.URL, reader)
	if err != nil {
		return CanonicalResponse{}, err
	}
	for name, value := range req.Headers {
		httpReq.Header.Set(name, value)
	}
	httpRes, err := client.Do(httpReq)
	if err != nil {
		return CanonicalResponse{}, err
	}
	defer httpRes.Body.Close()
	// The body read can fail mid-stream too, so it shares the transport error path.
	data, err := io.ReadAll(httpRes.Body)
	if err != nil {
		return CanonicalResponse{}, err
	}
	headers := make(map[string]string, len(httpRes.Header))
	for name := range httpRes.Header {
		headers[strings.ToLower(name)] = httpRes.Header.Get(name)
	}
	return CanonicalResponse{Status: httpRes.StatusCode, Headers: headers, Body: string(data)}, nil
}

// isSuccessStatus: a 2xx is a success even when its exact code was not the one
// declared (a server may answer 200 where 201 was declared); a declared
// success code outside the 2xx range still counts.
func isSuccessStatus(d *WireDescriptor, status int) bool {
	if status >= 200 && status < 300 {
		return true
	}
	for _, s := range d.Success {
		if s.Status == status {
			return true
		}
	}
	return false
}

// applyResponseBindings reads the response-bound members off the (canonical)
// response (a header value, or the status code) and folds them into the
// decoded body so the generated decoder sees them as ordinary fields. Applied
// only on success; interpreting the descriptor is the runtime's job, which
// keeps the generated client blind.
func applyResponseBindings(d *WireDescriptor, res CanonicalResponse) string {
	if len(d.ResponseBindings) == 0 {
		return res.Body
	}
	object := map[string]any{}
	// A non-object or empty body leaves the response-bound fields to stand on
	// their own.
	_ = json.Unmarshal([]byte(res.Body), &object)
	for _, b := range d.ResponseBindings {
		if b.Part.Kind == "statusCode" {
			object[b.Member] = res.Status
			continue
		}
		// Header names are case-insensitive; response header keys are lowercased.
		if value, ok := res.Headers[strings.ToLower(b.Part.Name)]; ok {
			object[b.Member] = value
		} else {
			object[b.Member] = nil
		}
	}
	// Every value in the object came off decoded JSON, an int status, or a
	// header string, so re-marshalling cannot fail.
	folded, _ := json.Marshal(object)
	return string(folded)
}
