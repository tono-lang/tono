// Package mathkit is a stand-in for a third-party numeric library with the
// shapes real Go libraries have, kept deliberately (this is the FFI bench,
// not a fixture bent to fit the emitter): the handle is an interface, not a
// pointer to a struct; the constructors are generic over the value they
// produce; the formula constructor takes functional variadic options; and
// the fallback constructor composes handles it already built.
package mathkit

import (
	"context"
	"errors"
	"fmt"
	"math"
	"os"
	"reflect"
	"strconv"
	"strings"
)

// Calculator produces one value. Every constructor below returns this
// interface, never a concrete type, so a generated SDK must hold and pass
// the interface value itself.
type Calculator[T any] interface {
	Compute(ctx context.Context) (T, error)
	Close() error
}

// Option configures FromFormula. Options are functional and variadic, the
// idiomatic Go shape; there is no options struct.
type Option func(*settings)

type settings struct {
	precision int
}

// WithPrecision rounds the formula result to the given number of digits.
func WithPrecision(digits int) Option {
	return func(s *settings) { s.precision = digits }
}

// ErrNoStrategy is returned when FromFallback receives a strategy it does
// not know.
var ErrNoStrategy = errors.New("mathkit: unknown fallback strategy")

// ErrParse is wrapped by every formula that does not parse, so a caller
// recognises the failure with errors.Is.
var ErrParse = errors.New("mathkit: cannot parse expression")

type constant[T any] struct{ value T }

func (c *constant[T]) Compute(ctx context.Context) (T, error) { return c.value, nil }
func (c *constant[T]) Close() error                           { return nil }

// FromConstant answers the same value on every Compute.
func FromConstant[T any](value T) (Calculator[T], error) {
	return &constant[T]{value: value}, nil
}

type formula[T any] struct {
	expr      string
	precision int
}

// Compute evaluates "<a> <op> <b>" for float64 results (the only
// instantiation the bench exercises); any other T fails loudly.
func (f *formula[T]) Compute(ctx context.Context) (T, error) {
	var zero T
	fields := strings.Fields(f.expr)
	if len(fields) != 3 {
		return zero, fmt.Errorf("%w: %q", ErrParse, f.expr)
	}
	a, err := strconv.ParseFloat(fields[0], 64)
	if err != nil {
		return zero, err
	}
	b, err := strconv.ParseFloat(fields[2], 64)
	if err != nil {
		return zero, err
	}
	var out float64
	switch fields[1] {
	case "+":
		out = a + b
	case "-":
		out = a - b
	case "*":
		out = a * b
	case "/":
		out = a / b
	default:
		return zero, fmt.Errorf("mathkit: unknown operator %q", fields[1])
	}
	if f.precision > 0 {
		scale := math.Pow(10, float64(f.precision))
		out = math.Round(out*scale) / scale
	}
	v, ok := any(out).(T)
	if !ok {
		return zero, fmt.Errorf("mathkit: formula results are float64, not %T", zero)
	}
	return v, nil
}
func (f *formula[T]) Close() error { return nil }

// FromFormula evaluates an expression, shaped by zero or more options.
func FromFormula[T any](expr string, opts ...Option) (Calculator[T], error) {
	s := settings{}
	for _, o := range opts {
		o(&s)
	}
	return &formula[T]{expr: expr, precision: s.precision}, nil
}

type series[T any] struct{ values []T }

// Compute answers the last value of the series.
func (s *series[T]) Compute(ctx context.Context) (T, error) {
	var zero T
	if len(s.values) == 0 {
		return zero, errors.New("mathkit: empty series")
	}
	return s.values[len(s.values)-1], nil
}
func (s *series[T]) Close() error { return nil }

// FromSeries answers from a collection of values.
func FromSeries[T any](values []T) (Calculator[T], error) {
	return &series[T]{values: values}, nil
}

type fallback[T any] struct {
	strategy string
	calcs    []Calculator[T]
}

// Compute asks each composed calculator in turn ("first" answers the first
// success, "last" the last one).
func (f *fallback[T]) Compute(ctx context.Context) (T, error) {
	var zero T
	var last T
	var lastErr error = errors.New("mathkit: no calculators to fall back to")
	for _, c := range f.calcs {
		v, err := c.Compute(ctx)
		if err != nil {
			lastErr = err
			continue
		}
		if f.strategy == "first" {
			return v, nil
		}
		last, lastErr = v, nil
	}
	if lastErr != nil {
		return zero, lastErr
	}
	return last, nil
}

func (f *fallback[T]) Close() error {
	for _, c := range f.calcs {
		if err := c.Close(); err != nil {
			return err
		}
	}
	return nil
}

// FromFallback composes calculators the caller already built: the library
// takes its own handles back as arguments, variadically.
func FromFallback[T any](strategy string, calcs ...Calculator[T]) (Calculator[T], error) {
	if strategy != "first" && strategy != "last" {
		return nil, ErrNoStrategy
	}
	return &fallback[T]{strategy: strategy, calcs: calcs}, nil
}

// Client is a session with a remote calculation service. Its name is the
// one every generated SDK's entry takes as well, on purpose: a spelling
// naming it is the library's, and the generated code must keep the package
// selector on it.
type Client struct {
	addr     string
	greeting string
}

// Open starts a session with the service at addr.
func Open(addr string) (*Client, error) {
	return &Client{addr: addr}, nil
}

// Dial starts a session with the service at addr and cannot fail: it
// returns only the handle, the shape of a Go client constructor that
// connects lazily. A generated SDK must bind one value here, never
// (T, error), while Open next to it keeps the error.
func Dial(addr string) *Client {
	return &Client{addr: addr}
}

// Ping answers the service's greeting ("pong" unless the session was
// opened with one of its own).
func (c *Client) Ping() (string, error) {
	greeting := c.greeting
	if greeting == "" {
		greeting = "pong"
	}
	return greeting + " from " + c.addr, nil
}

// Memo keeps one value of the caller's own type: the library is generic
// over a type it never sees, the way a settings or cache library is.
type Memo[T any] struct{ value T }

// Remember keeps value for a later Recall.
func Remember[T any](value T) (*Memo[T], error) {
	return &Memo[T]{value: value}, nil
}

// Recall answers the value Remember kept.
func (m *Memo[T]) Recall(ctx context.Context) (T, error) {
	return m.value, nil
}

// Options configures a session. Connect takes it by pointer, the way a Go
// client library takes its options struct: a generated SDK must pass the
// address of the literal it builds, while the type itself stays Options.
type Options struct {
	Addr     string
	Greeting string
}

// Connect starts a session with the service opt names. The session keeps
// what it read from opt, not opt itself: every literal a caller builds is
// its own address, and two sessions never share one.
func Connect(opt *Options) (*Client, error) {
	if opt == nil {
		return nil, errors.New("mathkit: nil options")
	}
	return &Client{addr: opt.Addr, greeting: opt.Greeting}, nil
}

// ErrMissing is the error a Reading carries for a key the session has no
// value for.
var ErrMissing = errors.New("mathkit: no such key")

// Reading is what Read answers: the value and the error travel inside it
// and are read later through Result, the command shape of Go clients. The
// call itself returns only the object, never (T, error).
type Reading struct {
	value string
	err   error
}

// Result answers what Read found, or the error it hit.
func (r *Reading) Result() (string, error) {
	return r.value, r.err
}

// Read looks key up in the session. It never fails by itself: the outcome,
// value or error, is inside the Reading, so a generated SDK must call
// Result on what Read returned to reach either.
func (c *Client) Read(ctx context.Context, key string) *Reading {
	switch key {
	case "addr":
		return &Reading{value: c.addr}
	case "greeting":
		greeting := c.greeting
		if greeting == "" {
			greeting = "pong"
		}
		return &Reading{value: greeting}
	default:
		return &Reading{err: fmt.Errorf("%w: %q", ErrMissing, key)}
	}
}

// Tuning is a calculator's calibration resolved for the caller's own struct
// T: the library reads T by reflection, from the `env:"..."` tag on each of
// its fields, the shape of a configuration library. The interface is held
// by value, never as a pointer, and the constructors are generic over a
// type the library never sees.
type Tuning[T any] interface {
	Load(ctx context.Context) (T, error)
}

// EnvOpt configures TuningFromEnv: functional and variadic, the same shape
// Option has on FromFormula.
type EnvOpt func(*envParams)

type envParams struct{ params map[string]string }

// WithParam substitutes {name} inside every variable name a tag declares,
// at run time: with `env:"CALC_{profile}_SCALE"` on the field,
// WithParam("profile", "alpha") reads CALC_alpha_SCALE.
func WithParam(name, value string) EnvOpt {
	return func(p *envParams) { p.params[name] = value }
}

// ErrUnset is the error Load carries for a variable the environment lacks.
var ErrUnset = errors.New("mathkit: variable not set")

type envTuning[T any] struct{ params map[string]string }

// TuningFromEnv resolves T from the environment. Every field of T must
// carry an `env` tag naming its variable, with {service} and every
// WithParam name substituted; T itself must be a struct. A field with no
// tag is the defect this contract exists for: the library cannot know
// where the value comes from, and Load fails at run time, exactly as a
// generated type without tags fails against a reflection-driven library
// after compiling cleanly.
func TuningFromEnv[T any](service string, opts ...EnvOpt) (Tuning[T], error) {
	var zero T
	if reflect.TypeOf(zero) == nil || reflect.TypeOf(zero).Kind() != reflect.Struct {
		return nil, fmt.Errorf("mathkit: a tuning resolves a struct, not %T", zero)
	}
	p := envParams{params: map[string]string{"service": service}}
	for _, o := range opts {
		o(&p)
	}
	return &envTuning[T]{params: p.params}, nil
}

// Load reads every field of T from its variable, substituting the
// parameters in the name first.
func (e *envTuning[T]) Load(ctx context.Context) (T, error) {
	var out T
	v := reflect.ValueOf(&out).Elem()
	t := v.Type()
	for i := 0; i < t.NumField(); i++ {
		f := t.Field(i)
		name, ok := f.Tag.Lookup("env")
		if !ok {
			return out, fmt.Errorf("mathkit: field %s of %s carries no env tag", f.Name, t.Name())
		}
		for k, val := range e.params {
			name = strings.ReplaceAll(name, "{"+k+"}", val)
		}
		raw, found := os.LookupEnv(name)
		if !found {
			return out, fmt.Errorf("%w: %s", ErrUnset, name)
		}
		switch f.Type.Kind() {
		case reflect.String:
			v.Field(i).SetString(raw)
		case reflect.Float64:
			x, err := strconv.ParseFloat(raw, 64)
			if err != nil {
				return out, err
			}
			v.Field(i).SetFloat(x)
		case reflect.Int, reflect.Int64:
			x, err := strconv.ParseInt(raw, 10, 64)
			if err != nil {
				return out, err
			}
			v.Field(i).SetInt(x)
		case reflect.Bool:
			x, err := strconv.ParseBool(raw)
			if err != nil {
				return out, err
			}
			v.Field(i).SetBool(x)
		default:
			return out, fmt.Errorf("mathkit: cannot read a %s from the environment", f.Type)
		}
	}
	return out, nil
}

type pinnedTuning[T any] struct{ value T }

func (p *pinnedTuning[T]) Load(ctx context.Context) (T, error) { return p.value, nil }

// TuningPinned answers the value it was given: the defaults a calibration
// falls back to when the environment does not carry it.
func TuningPinned[T any](value T) (Tuning[T], error) {
	return &pinnedTuning[T]{value: value}, nil
}

type tuningFallback[T any] struct{ tunings []Tuning[T] }

// Load asks each composed tuning in turn and answers the first that loads.
func (f *tuningFallback[T]) Load(ctx context.Context) (T, error) {
	var zero T
	var lastErr error = errors.New("mathkit: no tunings to fall back to")
	for _, t := range f.tunings {
		v, err := t.Load(ctx)
		if err == nil {
			return v, nil
		}
		lastErr = err
	}
	return zero, lastErr
}

// TuningFallback composes tunings the caller already built, variadically,
// the same shape FromFallback has for calculators.
func TuningFallback[T any](tunings ...Tuning[T]) (Tuning[T], error) {
	return &tuningFallback[T]{tunings: tunings}, nil
}
