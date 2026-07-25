package tonohttp

import (
	"testing"
)

func desc(mutate func(*WireDescriptor)) *WireDescriptor {
	d := &WireDescriptor{
		HTTPMethod: "POST",
		URI:        "/things",
		Success:    []SuccessCase{{Status: 200}},
	}
	if mutate != nil {
		mutate(d)
	}
	return d
}

func binding(member, kind, name string) Binding {
	return Binding{Member: member, Part: Part{Kind: kind, Name: name}}
}

func TestBuildPath(t *testing.T) {
	d := desc(func(d *WireDescriptor) {
		d.URI = "/things/{id}"
		d.Bindings = []Binding{binding("id", "label", "")}
	})
	if got := buildPath(d, map[string]any{"id": "abc"}); got != "/things/abc" {
		t.Fatalf("present label: %q", got)
	}
	if got := buildPath(d, map[string]any{}); got != "/things/" {
		t.Fatalf("absent label substitutes empty: %q", got)
	}
	if got := buildPath(d, map[string]any{"id": nil}); got != "/things/" {
		t.Fatalf("null label substitutes empty: %q", got)
	}
	if got := buildPath(d, map[string]any{"id": "a/b c"}); got != "/things/a%2Fb%20c" {
		t.Fatalf("label escapes: %q", got)
	}
	if got := buildPath(d, map[string]any{"id": float64(42)}); got != "/things/42" {
		t.Fatalf("integral float renders without decimals: %q", got)
	}
}

func TestBuildPathIgnoresNonLabelBindings(t *testing.T) {
	d := desc(func(d *WireDescriptor) {
		d.URI = "/things/{q}"
		d.Bindings = []Binding{binding("q", "query", "q")}
	})
	// A query binding whose name matches a path placeholder is left literal.
	if got := buildPath(d, map[string]any{"q": "v"}); got != "/things/{q}" {
		t.Fatalf("non-label substituted: %q", got)
	}
}

func TestBuildQuery(t *testing.T) {
	d := desc(func(d *WireDescriptor) {
		d.Bindings = []Binding{binding("q", "query", "q"), binding("tag", "query", "tag")}
	})
	got := buildQuery(d, map[string]any{"q": "hi", "tag": []any{"a", "b"}})
	if got != "q=hi&tag=a&tag=b" {
		t.Fatalf("list repeats per element: %q", got)
	}
	if got := buildQuery(d, map[string]any{"q": nil}); got != "" {
		t.Fatalf("null omitted: %q", got)
	}
	if got := buildQuery(d, map[string]any{"tag": "kept"}); got != "tag=kept" {
		t.Fatalf("absent omitted, present kept: %q", got)
	}
	if got := buildQuery(d, map[string]any{"q": "a b", "tag": true}); got != "q=a+b&tag=true" {
		t.Fatalf("escaping and scalar formats: %q", got)
	}
	if got := buildQuery(d, map[string]any{"q": 3.5}); got != "q=3.5" {
		t.Fatalf("fractional float: %q", got)
	}
}

func TestBuildQueryIgnoresNonQueryBindings(t *testing.T) {
	d := desc(func(d *WireDescriptor) {
		d.Bindings = []Binding{binding("h", "header", "H")}
	})
	if got := buildQuery(d, map[string]any{"h": "v"}); got != "" {
		t.Fatalf("header leaked into query: %q", got)
	}
}

func TestBuildHeaders(t *testing.T) {
	d := desc(func(d *WireDescriptor) {
		d.Bindings = []Binding{binding("trace", "header", "X-Trace"), binding("drop", "header", "X-Drop"), binding("q", "query", "q")}
	})
	base := map[string]string{"Authorization": "Bearer t"}
	headers := buildHeaders(d, map[string]any{"trace": "t1", "drop": nil, "q": "v"}, base)
	if headers["X-Trace"] != "t1" {
		t.Fatalf("bound header missing: %+v", headers)
	}
	if _, ok := headers["X-Drop"]; ok {
		t.Fatalf("null header sent: %+v", headers)
	}
	if _, ok := headers["q"]; ok {
		t.Fatalf("query leaked into headers: %+v", headers)
	}
	if headers["Authorization"] != "Bearer t" {
		t.Fatalf("base header lost: %+v", headers)
	}
	if base["X-Trace"] != "" {
		t.Fatal("base map mutated")
	}
}

func TestBuildBodyAssemblesObjectInBindingOrder(t *testing.T) {
	d := desc(func(d *WireDescriptor) {
		d.Bindings = []Binding{binding("b", "body", ""), binding("a", "body", ""), binding("skip", "query", "skip")}
	})
	body, err := buildBody(d, map[string]any{"a": float64(1), "b": "two", "skip": "no"})
	if err != nil {
		t.Fatalf("build: %v", err)
	}
	if string(body) != `{"b":"two","a":1}` {
		t.Fatalf("body: %s", body)
	}
}

func TestBuildBodyMemberPresence(t *testing.T) {
	d := desc(func(d *WireDescriptor) {
		d.Bindings = []Binding{binding("a", "body", ""), binding("b", "body", "")}
	})
	body, err := buildBody(d, map[string]any{"a": float64(1)})
	if err != nil || string(body) != `{"a":1}` {
		t.Fatalf("absent member kept: %s %v", body, err)
	}
	body, err = buildBody(d, map[string]any{"a": nil})
	if err != nil || string(body) != `{"a":null}` {
		t.Fatalf("present null must land as null: %s %v", body, err)
	}
	body, err = buildBody(d, map[string]any{})
	if err != nil || body != nil {
		t.Fatalf("no members must mean no body: %s %v", body, err)
	}
	body, err = buildBody(d, nil)
	if err != nil || body != nil {
		t.Fatalf("nil input must mean no body: %s %v", body, err)
	}
}

func TestBuildBodyPayload(t *testing.T) {
	d := desc(func(d *WireDescriptor) {
		d.Bindings = []Binding{binding("raw", "payload", "")}
	})
	body, err := buildBody(d, map[string]any{"raw": map[string]any{"nested": true}})
	if err != nil || string(body) != `{"nested":true}` {
		t.Fatalf("payload is the whole body, no envelope: %s %v", body, err)
	}
	body, err = buildBody(d, map[string]any{})
	if err != nil || body != nil {
		t.Fatalf("absent payload means no body: %s %v", body, err)
	}
}

func TestBuildBodyUnencodableValue(t *testing.T) {
	d := desc(func(d *WireDescriptor) {
		d.Bindings = []Binding{binding("a", "body", "")}
	})
	if _, err := buildBody(d, map[string]any{"a": make(chan int)}); err == nil {
		t.Fatal("unencodable value must error")
	}
	d = desc(func(d *WireDescriptor) {
		d.Bindings = []Binding{binding("raw", "payload", "")}
	})
	if _, err := buildBody(d, map[string]any{"raw": make(chan int)}); err == nil {
		t.Fatal("unencodable payload must error")
	}
}

func TestFormatScalar(t *testing.T) {
	cases := []struct {
		in   any
		want string
	}{
		{"s", "s"},
		{true, "true"},
		{false, "false"},
		{float64(3), "3"},
		{float64(3.5), "3.5"},
		{int(7), "7"},
		{int64(9), "9"},
		{[]any{1.0}, "[1]"},
		{make(chan int), ""},
	}
	for _, c := range cases {
		if got := formatScalar(c.in); got != c.want {
			t.Errorf("formatScalar(%v) = %q, want %q", c.in, got, c.want)
		}
	}
}

func TestHasHeader(t *testing.T) {
	headers := map[string]string{"Content-Type": "text/plain"}
	if !hasHeader(headers, "content-type") {
		t.Fatal("case-insensitive match missed")
	}
	if hasHeader(headers, "authorization") {
		t.Fatal("matched a header that is not there")
	}
}
