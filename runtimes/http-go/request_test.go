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
	if got := buildPath(d, map[string]any{"id": "abc"}, nil); got != "/things/abc" {
		t.Fatalf("present label: %q", got)
	}
	if got := buildPath(d, map[string]any{}, nil); got != "/things/" {
		t.Fatalf("absent label substitutes empty: %q", got)
	}
	if got := buildPath(d, map[string]any{"id": nil}, nil); got != "/things/" {
		t.Fatalf("null label substitutes empty: %q", got)
	}
	if got := buildPath(d, map[string]any{"id": "a/b c"}, nil); got != "/things/a%2Fb%20c" {
		t.Fatalf("label escapes: %q", got)
	}
	if got := buildPath(d, map[string]any{"id": float64(42)}, nil); got != "/things/42" {
		t.Fatalf("integral float renders without decimals: %q", got)
	}
}

func TestBuildPathIgnoresNonLabelBindings(t *testing.T) {
	d := desc(func(d *WireDescriptor) {
		d.URI = "/things/{q}"
		d.Bindings = []Binding{binding("q", "query", "q")}
	})
	// A query binding whose name matches a path placeholder is left literal.
	if got := buildPath(d, map[string]any{"q": "v"}, nil); got != "/things/{q}" {
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
	headers := buildHeaders(d, map[string]any{"trace": "t1", "drop": nil, "q": "v"}, base, nil)
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

func tpLit(s string) TemplatePart { return TemplatePart{Lit: &s} }

func tpInput(name string) TemplatePart { return TemplatePart{Input: &name} }

func TestBuildPathSubstitutesFieldPlaceholders(t *testing.T) {
	d := desc(func(d *WireDescriptor) {
		d.URI = "/v/{.tenant}/things/{id}"
		d.Bindings = []Binding{binding("id", "label", "")}
	})
	values := map[string]any{"tenant": "acme corp"}
	if got := buildPath(d, map[string]any{"id": "7"}, values); got != "/v/acme%20corp/things/7" {
		t.Fatalf("field placeholder: %q", got)
	}
	// An absent or null field substitutes empty, same rule as an absent label.
	if got := buildPath(d, map[string]any{"id": "7"}, nil); got != "/v//things/7" {
		t.Fatalf("absent field substitutes empty: %q", got)
	}
	if got := buildPath(d, map[string]any{"id": "7"}, map[string]any{"tenant": nil}); got != "/v//things/7" {
		t.Fatalf("null field substitutes empty: %q", got)
	}
}

func TestSubstituteFieldPlaceholders(t *testing.T) {
	values := map[string]any{"a.b": "x", "n": float64(2)}
	if got := substituteFieldPlaceholders("/p/{.a.b}/q/{.n}", values); got != "/p/x/q/2" {
		t.Fatalf("dotted path and number: %q", got)
	}
	if got := substituteFieldPlaceholders("/plain", values); got != "/plain" {
		t.Fatalf("no placeholder must pass through: %q", got)
	}
	// An unterminated placeholder is left verbatim rather than eaten.
	if got := substituteFieldPlaceholders("/p/{.open", values); got != "/p/{.open" {
		t.Fatalf("unterminated placeholder: %q", got)
	}
	// A label-style placeholder (no dot) is not a field placeholder.
	if got := substituteFieldPlaceholders("/p/{id}", values); got != "/p/{id}" {
		t.Fatalf("label placeholder untouched: %q", got)
	}
}

func TestResolveEndpoint(t *testing.T) {
	values := map[string]any{"endpoint": "https://api.acme.test", "conf.url": "https://c", "num": float64(3)}
	if got, ok := resolveEndpoint([]string{"endpoint"}, values); !ok || got != "https://api.acme.test" {
		t.Fatalf("endpoint ref: %q %v", got, ok)
	}
	if got, ok := resolveEndpoint([]string{"conf", "url"}, values); !ok || got != "https://c" {
		t.Fatalf("nested endpoint ref joins with dots: %q %v", got, ok)
	}
	if _, ok := resolveEndpoint(nil, values); ok {
		t.Fatal("absent declaration must not resolve")
	}
	if _, ok := resolveEndpoint([]string{"missing"}, values); ok {
		t.Fatal("missing value must not resolve")
	}
	if _, ok := resolveEndpoint([]string{"num"}, values); ok {
		t.Fatal("non-string value must not resolve")
	}
	if _, ok := resolveEndpoint([]string{"endpoint"}, map[string]any{"endpoint": ""}); ok {
		t.Fatal("empty string must not resolve")
	}
}

func TestResolveTemplate(t *testing.T) {
	values := map[string]any{"key": "K", "a.b": float64(4)}
	record := map[string]any{"id": "i9"}
	got, ok := resolveTemplate(
		[]TemplatePart{tpLit("X-"), {Field: []string{"key"}}, tpLit("-"), {Field: []string{"a", "b"}}, tpLit("-"), tpInput("id")},
		values, record,
	)
	if !ok || got != "X-K-4-i9" {
		t.Fatalf("template: %q %v", got, ok)
	}
	if _, ok := resolveTemplate([]TemplatePart{{Field: []string{"missing"}}}, values, record); ok {
		t.Fatal("missing field must fail the whole template")
	}
	if _, ok := resolveTemplate([]TemplatePart{tpInput("missing")}, values, record); ok {
		t.Fatal("missing input must fail the whole template")
	}
	if _, ok := resolveTemplate([]TemplatePart{{}}, values, record); ok {
		t.Fatal("an empty part must fail")
	}
	if _, ok := resolveTemplate([]TemplatePart{{Field: []string{"nil"}}}, map[string]any{"nil": nil}, record); ok {
		t.Fatal("null field must fail the whole template")
	}
}

func TestResolveValueExpr(t *testing.T) {
	values := map[string]any{"token": "t0"}
	if got, ok := resolveValueExpr(ValueExpr{Lit: "v"}, values, nil); !ok || got != "v" {
		t.Fatalf("lit: %q %v", got, ok)
	}
	if got, ok := resolveValueExpr(ValueExpr{Field: []string{"token"}}, values, nil); !ok || got != "t0" {
		t.Fatalf("field: %q %v", got, ok)
	}
	if got, ok := resolveValueExpr(ValueExpr{Template: []TemplatePart{tpLit("Bearer "), {Field: []string{"token"}}}}, values, nil); !ok || got != "Bearer t0" {
		t.Fatalf("template: %q %v", got, ok)
	}
	if _, ok := resolveValueExpr(ValueExpr{Field: []string{"missing"}}, values, nil); ok {
		t.Fatal("missing field must not resolve")
	}
	if _, ok := resolveValueExpr(ValueExpr{}, values, nil); ok {
		t.Fatal("an empty expression must not resolve")
	}
}

func TestBuildHeadersLayersDeclaredUnderBaseUnderBindings(t *testing.T) {
	d := desc(func(d *WireDescriptor) {
		d.Bindings = []Binding{binding("trace", "header", "X-Trace")}
		d.RequestHeaders = []RequestHeader{
			{Key: []TemplatePart{tpLit("X-Client")}, Value: ValueExpr{Field: []string{"client_name"}}},
			{Key: []TemplatePart{tpLit("Authorization")}, Value: ValueExpr{Lit: "declared"}},
			{Key: []TemplatePart{tpLit("X-Trace")}, Value: ValueExpr{Lit: "declared"}},
			{Key: []TemplatePart{tpLit("X-Skipped")}, Value: ValueExpr{Field: []string{"missing"}}},
			{Key: []TemplatePart{{Field: []string{"missing"}}}, Value: ValueExpr{Lit: "v"}},
			{Key: []TemplatePart{}, Value: ValueExpr{Lit: "empty key"}},
		}
	})
	values := map[string]any{"client_name": "demo"}
	base := map[string]string{"Authorization": "Bearer caller"}
	headers := buildHeaders(d, map[string]any{"trace": "t1"}, base, values)
	if headers["X-Client"] != "demo" {
		t.Fatalf("declared header missing: %+v", headers)
	}
	// The caller's base header wins over the declared one (bespoke wins).
	if headers["Authorization"] != "Bearer caller" {
		t.Fatalf("base must override declared: %+v", headers)
	}
	// The per-call binding is the most specific and wins over declared.
	if headers["X-Trace"] != "t1" {
		t.Fatalf("binding must override declared: %+v", headers)
	}
	if _, ok := headers["X-Skipped"]; ok {
		t.Fatalf("unresolvable declared value must omit the header: %+v", headers)
	}
	if len(headers) != 3 {
		t.Fatalf("unexpected headers: %+v", headers)
	}
}

func TestDeclaredHeadersEmptyDeclaration(t *testing.T) {
	if got := declaredHeaders(desc(nil), nil, nil); got != nil {
		t.Fatalf("no declaration must yield nil: %+v", got)
	}
}
