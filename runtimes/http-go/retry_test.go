package tonohttp

import (
	"context"
	"encoding/json"
	"errors"
	"testing"
	"time"
)

func lit(f float64) *float64 { return &f }
func ref(s string) *string   { return &s }

// A runtime with deterministic seams: jitter pinned to 0.5 and a recording,
// instant sleep.
func deterministic(t *testing.T, opts Options) (*Runtime, *[]time.Duration) {
	t.Helper()
	r := newRuntime(t, opts)
	var delays []time.Duration
	r.randFloat = func() float64 { return 0.5 }
	r.sleep = func(ctx context.Context, d time.Duration) error {
		delays = append(delays, d)
		return nil
	}
	return r, &delays
}

func TestResolveMaxRetries(t *testing.T) {
	values := map[string]any{"f": float64(2), "i": int(3), "i64": int64(4), "s": "5"}
	cases := []struct {
		name string
		spec *RetrySpec
		want int
	}{
		{"nil spec", nil, 0},
		{"literal", &RetrySpec{Max: ValueSource{Lit: lit(2)}}, 2},
		{"fractional floors", &RetrySpec{Max: ValueSource{Lit: lit(2.9)}}, 2},
		{"below one disables", &RetrySpec{Max: ValueSource{Lit: lit(0.5)}}, 0},
		{"zero", &RetrySpec{Max: ValueSource{Lit: lit(0)}}, 0},
		{"negative", &RetrySpec{Max: ValueSource{Lit: lit(-3)}}, 0},
		{"ref float", &RetrySpec{Max: ValueSource{Ref: ref("f")}}, 2},
		{"ref int", &RetrySpec{Max: ValueSource{Ref: ref("i")}}, 3},
		{"ref int64", &RetrySpec{Max: ValueSource{Ref: ref("i64")}}, 4},
		{"ref to non-number", &RetrySpec{Max: ValueSource{Ref: ref("s")}}, 0},
		{"ref to nothing", &RetrySpec{Max: ValueSource{Ref: ref("absent")}}, 0},
		{"empty source", &RetrySpec{}, 0},
	}
	for _, c := range cases {
		if got := resolveMaxRetries(c.spec, values); got != c.want {
			t.Errorf("%s: got %d, want %d", c.name, got, c.want)
		}
	}
}

func TestResolveTimeout(t *testing.T) {
	values := map[string]any{"t": float64(250)}
	if got := resolveTimeout(nil, values); got != 0 {
		t.Fatalf("nil source: %v", got)
	}
	if got := resolveTimeout(&ValueSource{Lit: lit(1500)}, values); got != 1500*time.Millisecond {
		t.Fatalf("literal: %v", got)
	}
	if got := resolveTimeout(&ValueSource{Ref: ref("t")}, values); got != 250*time.Millisecond {
		t.Fatalf("ref: %v", got)
	}
	if got := resolveTimeout(&ValueSource{Ref: ref("absent")}, values); got != 0 {
		t.Fatalf("unresolved ref: %v", got)
	}
}

func TestBackoffDelay(t *testing.T) {
	cases := []struct {
		retry  int
		random float64
		want   time.Duration
	}{
		{0, 0.5, 50 * time.Millisecond},
		{1, 0.5, 100 * time.Millisecond},
		{2, 0.5, 200 * time.Millisecond},
		{3, 0.5, 400 * time.Millisecond},
		{4, 0.5, 800 * time.Millisecond},
		// 100 * 2^5 = 3200 caps at 2000.
		{5, 0.5, 1000 * time.Millisecond},
		{9, 1.0, 2000 * time.Millisecond},
		{3, 0, 0},
	}
	for _, c := range cases {
		if got := backoffDelay(c.retry, c.random); got != c.want {
			t.Errorf("backoffDelay(%d, %v) = %v, want %v", c.retry, c.random, got, c.want)
		}
	}
}

// declaredErrorRule is one entry of a stand-in for a generated
// Decode<Op>Error + Retryable() pair: a status, an optional @errorCode
// discriminator, and whether that error is retryable.
type declaredErrorRule struct {
	Status    int
	Code      *string
	Retryable bool
}

// classifyLikeDiscriminator builds the predicate a generated client passes to
// Execute: the first status match with an agreeing code decides (a declared
// code must equal the body's "code" field; a null code matches any body),
// exactly the ordering discrimination_order gives the real emitter.
func classifyLikeDiscriminator(declared []declaredErrorRule) func(status int, body string) bool {
	return func(status int, body string) bool {
		code := bodyCodeForTest(body)
		for _, e := range declared {
			if e.Status != status {
				continue
			}
			if e.Code == nil || (code != nil && *code == *e.Code) {
				return e.Retryable
			}
		}
		return false
	}
}

func bodyCodeForTest(body string) *string {
	var object map[string]any
	if err := json.Unmarshal([]byte(body), &object); err != nil {
		return nil
	}
	if code, ok := object["code"].(string); ok {
		return &code
	}
	return nil
}

func TestIsRetryableClassification(t *testing.T) {
	retryable := classifyLikeDiscriminator([]declaredErrorRule{
		{Status: 429, Code: ref("overloaded"), Retryable: true},
		{Status: 429, Code: ref("quota"), Retryable: false},
		{Status: 503, Code: nil, Retryable: true},
		{Status: 404, Code: nil, Retryable: false},
	})
	if !isRetryable(Outcome{Kind: OutcomeTransport}, retryable) {
		t.Fatal("transport failures always retry")
	}
	if isRetryable(Outcome{Kind: OutcomeSuccess, Status: 200}, retryable) {
		t.Fatal("a success never retries")
	}
	if !isRetryable(Outcome{Kind: OutcomeError, Status: 429, Body: `{"code":"overloaded"}`}, retryable) {
		t.Fatal("retryable code match must retry")
	}
	if isRetryable(Outcome{Kind: OutcomeError, Status: 429, Body: `{"code":"quota"}`}, retryable) {
		t.Fatal("non-retryable code match must not retry")
	}
	if isRetryable(Outcome{Kind: OutcomeError, Status: 429, Body: `{"code":"other"}`}, retryable) {
		t.Fatal("an unmatched code must not retry")
	}
	if isRetryable(Outcome{Kind: OutcomeError, Status: 429, Body: "not json"}, retryable) {
		t.Fatal("a body without a code cannot match a coded error")
	}
	if !isRetryable(Outcome{Kind: OutcomeError, Status: 503, Body: "not json"}, retryable) {
		t.Fatal("a null code matches any body")
	}
	if isRetryable(Outcome{Kind: OutcomeError, Status: 404, Body: "{}"}, retryable) {
		t.Fatal("declared non-retryable must not retry")
	}
	if isRetryable(Outcome{Kind: OutcomeError, Status: 500, Body: "{}"}, retryable) {
		t.Fatal("an undeclared status must not retry")
	}
	if isRetryable(Outcome{Kind: OutcomeError, Status: 429, Body: `{"code":5}`}, retryable) {
		t.Fatal("a non-string code field cannot match")
	}
	if isRetryable(Outcome{Kind: OutcomeError, Status: 500, Body: "{}"}, nil) {
		t.Fatal("a nil predicate (no declared errors) must not retry")
	}
}

func TestRetryLoopHonorsMaxAndBackoff(t *testing.T) {
	attempts := 0
	transport := func(ctx context.Context, req CanonicalRequest) (CanonicalResponse, error) {
		attempts++
		return CanonicalResponse{}, errors.New("down")
	}
	r, delays := deterministic(t, Options{BaseURL: "https://api.test", Transport: transport})
	d := desc(func(d *WireDescriptor) { d.Retry = &RetrySpec{Max: ValueSource{Lit: lit(3)}} })
	outcome, err := r.Execute(context.Background(), d, nil, nil, nil)
	if err != nil || outcome.Kind != OutcomeTransport {
		t.Fatalf("exhausted retries must return the last outcome: %+v %v", outcome, err)
	}
	if attempts != 4 {
		t.Fatalf("attempts: %d", attempts)
	}
	want := []time.Duration{50 * time.Millisecond, 100 * time.Millisecond, 200 * time.Millisecond}
	if len(*delays) != len(want) {
		t.Fatalf("delays: %v", *delays)
	}
	for i, w := range want {
		if (*delays)[i] != w {
			t.Fatalf("delay %d: %v, want %v", i, (*delays)[i], w)
		}
	}
}

func TestRetryStopsAtFirstNonRetryableOutcome(t *testing.T) {
	responses := []CanonicalResponse{
		{Status: 503, Body: `{}`},
		{Status: 404, Body: `{}`},
		{Status: 200, Body: `{}`},
	}
	attempts := 0
	transport := func(ctx context.Context, req CanonicalRequest) (CanonicalResponse, error) {
		res := responses[attempts]
		attempts++
		return res, nil
	}
	r, delays := deterministic(t, Options{BaseURL: "https://api.test", Transport: transport})
	d := desc(func(d *WireDescriptor) {
		d.Retry = &RetrySpec{Max: ValueSource{Lit: lit(5)}}
	})
	retryable := func(status int, body string) bool { return status == 503 }
	outcome, err := r.Execute(context.Background(), d, nil, nil, retryable)
	if err != nil || outcome.Kind != OutcomeError || outcome.Status != 404 {
		t.Fatalf("first non-retryable outcome must surface: %+v %v", outcome, err)
	}
	if attempts != 2 || len(*delays) != 1 {
		t.Fatalf("attempts %d, delays %v", attempts, *delays)
	}
}

func TestPerAttemptTimeoutRetriesAndRecovers(t *testing.T) {
	attempts := 0
	transport := func(ctx context.Context, req CanonicalRequest) (CanonicalResponse, error) {
		attempts++
		if attempts == 1 {
			// Honor the ctx deadline, as the canonical transport contract
			// asks; bail out fast if a mutant keeps the deadline from firing.
			select {
			case <-ctx.Done():
				return CanonicalResponse{}, ctx.Err()
			case <-time.After(2 * time.Second):
				return CanonicalResponse{}, errors.New("the per-attempt deadline never fired")
			}
		}
		return CanonicalResponse{Status: 200, Body: "{}"}, nil
	}
	r, delays := deterministic(t, Options{BaseURL: "https://api.test", Transport: transport})
	d := desc(func(d *WireDescriptor) {
		d.Retry = &RetrySpec{Max: ValueSource{Lit: lit(1)}}
		d.Timeout = &ValueSource{Lit: lit(20)}
	})
	start := time.Now()
	outcome, err := r.Execute(context.Background(), d, nil, nil, nil)
	if err != nil || outcome.Kind != OutcomeSuccess {
		t.Fatalf("timeout must count as transport failure and retry: %+v %v", outcome, err)
	}
	if attempts != 2 || len(*delays) != 1 {
		t.Fatalf("attempts %d, delays %v", attempts, *delays)
	}
	if elapsed := time.Since(start); elapsed < 20*time.Millisecond {
		t.Fatalf("first attempt returned before its deadline: %v", elapsed)
	}
}

func TestCancellationDuringBackoffSurfacesAsTransport(t *testing.T) {
	transport := func(ctx context.Context, req CanonicalRequest) (CanonicalResponse, error) {
		return CanonicalResponse{}, errors.New("down")
	}
	r := newRuntime(t, Options{BaseURL: "https://api.test", Transport: transport})
	r.randFloat = func() float64 { return 0.5 }
	ctx, cancel := context.WithCancel(context.Background())
	r.sleep = func(ctx context.Context, d time.Duration) error {
		cancel()
		return sleepContext(ctx, d)
	}
	d := desc(func(d *WireDescriptor) { d.Retry = &RetrySpec{Max: ValueSource{Lit: lit(3)}} })
	outcome, err := r.Execute(ctx, d, nil, nil, nil)
	if err != nil || outcome.Kind != OutcomeTransport || !errors.Is(outcome.Cause, context.Canceled) {
		t.Fatalf("cancellation during backoff: %+v %v", outcome, err)
	}
}

func TestSleepContext(t *testing.T) {
	if err := sleepContext(context.Background(), time.Millisecond); err != nil {
		t.Fatalf("normal sleep: %v", err)
	}
	ctx, cancel := context.WithCancel(context.Background())
	cancel()
	// Half a second, not an hour: a mutant that ignores the cancellation then
	// fails fast on the return value instead of hanging the binary.
	if err := sleepContext(ctx, 500*time.Millisecond); !errors.Is(err, context.Canceled) {
		t.Fatalf("cancelled sleep: %v", err)
	}
}
