package tonohttp

import (
	"context"
	"errors"
	"math/rand"
	"net/http"
	"time"
)

// CanonicalRequest is the request the runtime builds from a descriptor before
// sending it. A BeforeRequest hook receives it and may return a mutated copy
// (set an auth header, sign the body); a canonical Transport receives it as the
// whole call input. Body is nil when the request carries no body.
type CanonicalRequest struct {
	Method  string
	URL     string
	Headers map[string]string
	Body    []byte
}

// CanonicalResponse is the response the runtime reads before classifying it.
// Header keys are lowercased (HTTP header names are case-insensitive). An
// AfterResponse hook may return a mutated copy; a canonical Transport produces
// one as the whole call output.
type CanonicalResponse struct {
	Status  int
	Headers map[string]string
	Body    string
}

// Transport is the canonical transport slot: adapt any HTTP stack by mapping
// CanonicalRequest/CanonicalResponse, without emulating *http.Client.
//
// Contract: one call is one attempt. The runtime owns retry (driven by the
// descriptor's retry declaration), so a transport with internal retries does
// not combine with it: either disable the transport's retries or do not
// declare retry on the operations. The transport must honor ctx cancellation;
// the per-attempt timeout arrives as a ctx deadline.
type Transport func(ctx context.Context, req CanonicalRequest) (CanonicalResponse, error)

// Options is what the caller supplies once, shared across every operation.
// Exactly one transport slot may be set: Client (native) or Transport
// (canonical); setting both is a construction error. Neither set defaults to
// http.DefaultClient. The runtime ships no auth of its own.
type Options struct {
	BaseURL   string
	Client    *http.Client
	Transport Transport
	Headers   map[string]string
	// Values are the resolved client fields the descriptor's ref positions
	// (retry max, timeout) look up by canonical name. The runtime stays blind
	// to where they came from.
	Values map[string]any
}

// Hooks are the lifecycle slots the runtime invokes around the transport, once
// per attempt. A hook error propagates raw (the generated wrapper owns turning
// it into its taxonomy); it is never misreported as a transport failure and is
// never retried.
type Hooks struct {
	BeforeRequest func(ctx context.Context, req CanonicalRequest) (CanonicalRequest, error)
	AfterResponse func(ctx context.Context, res CanonicalResponse) (CanonicalResponse, error)
}

// OutcomeKind discriminates the three raw results of a call.
type OutcomeKind string

// The three raw results: a success status, an error status, or a failure to
// obtain a response at all (network failure, timeout, cancellation).
const (
	OutcomeSuccess   OutcomeKind = "success"
	OutcomeError     OutcomeKind = "error"
	OutcomeTransport OutcomeKind = "transport"
)

// Outcome is the raw result of one call, deliberately free of any error
// taxonomy: the generated SDK maps each kind onto its own idiomatic form, so
// the runtime stays a thin, language-neutral transport. Status and Body are
// set for success/error outcomes; Cause for transport outcomes.
type Outcome struct {
	Kind   OutcomeKind
	Status int
	Body   string
	Cause  error
}

// Runtime executes wire descriptors against one configured transport.
type Runtime struct {
	opts Options
	// Deterministic seams: production uses the real clock and rand; tests pin
	// them to make retry timing and jitter assertable.
	sleep     func(ctx context.Context, d time.Duration) error
	randFloat func() float64
}

// New validates the options and builds a Runtime. Setting both transport slots
// is an error: the caller must pick the native slot or the canonical one.
func New(opts Options) (*Runtime, error) {
	if opts.Client != nil && opts.Transport != nil {
		return nil, errors.New("Options.Client and Options.Transport are mutually exclusive: set the native slot or the canonical slot, not both")
	}
	return &Runtime{
		opts:      opts,
		sleep:     sleepContext,
		randFloat: rand.Float64,
	}, nil
}

func sleepContext(ctx context.Context, d time.Duration) error {
	timer := time.NewTimer(d)
	defer timer.Stop()
	select {
	case <-ctx.Done():
		return ctx.Err()
	case <-timer.C:
		return nil
	}
}
