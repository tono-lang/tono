package tonohttp

import (
	"encoding/json"
	"net/url"
	"regexp"
	"strconv"
	"strings"
)

// The descriptor is executed verbatim: every binding was resolved once by the
// Protocol and frozen into the descriptor, so nothing here re-derives a
// default (an unmarked member is already a body entry, a label is already a
// label entry).

// formatScalar renders a decoded JSON value the way the wire expects it in a
// path, query, or header position. JSON numbers arrive as float64; an integral
// one must not print a trailing ".0".
func formatScalar(v any) string {
	switch x := v.(type) {
	case string:
		return x
	case bool:
		return strconv.FormatBool(x)
	case float64:
		return strconv.FormatFloat(x, 'f', -1, 64)
	case int:
		return strconv.Itoa(x)
	case int64:
		return strconv.FormatInt(x, 10)
	default:
		b, err := json.Marshal(v)
		if err != nil {
			return ""
		}
		return string(b)
	}
}

// buildPath substitutes each label binding into its {name} placeholder and
// each {.field} placeholder with the resolved client value under that dotted
// path in values. A path parameter must be present, so an absent or null one
// substitutes empty rather than a literal "null".
func buildPath(d *WireDescriptor, record map[string]any, values map[string]any) string {
	path := d.URI
	for _, b := range d.Bindings {
		if b.Part.Kind != "label" {
			continue
		}
		value := ""
		if v, ok := record[b.Member]; ok && v != nil {
			value = url.PathEscape(formatScalar(v))
		}
		path = strings.Replace(path, "{"+b.Member+"}", value, 1)
	}
	return substituteFieldPlaceholders(path, values)
}

// fieldPlaceholder matches one {.a.b} run in a path template; an unterminated
// or label-style ({name}) brace pair stays verbatim.
var fieldPlaceholder = regexp.MustCompile(`\{\.([^}]*)\}`)

// substituteFieldPlaceholders replaces every {.a.b} run in a path template with
// the value under "a.b" in the resolved client values, path-escaped. An absent
// or null value substitutes empty, the same rule as an absent label.
func substituteFieldPlaceholders(path string, values map[string]any) string {
	return fieldPlaceholder.ReplaceAllStringFunc(path, func(m string) string {
		if v, ok := values[m[2:len(m)-1]]; ok && v != nil {
			return url.PathEscape(formatScalar(v))
		}
		return ""
	})
}

// resolveEndpoint yields the operation's base URL from its endpoint field
// reference: the string value under the dotted path in the resolved client
// values. Absent declaration, absent value, or a non-string all yield false
// (the caller falls back to Options.BaseURL).
func resolveEndpoint(endpoint []string, values map[string]any) (string, bool) {
	if len(endpoint) == 0 {
		return "", false
	}
	s, ok := values[strings.Join(endpoint, ".")].(string)
	if !ok || s == "" {
		return "", false
	}
	return s, true
}

// resolveTemplate renders template parts into one string: literal runs
// verbatim, entry-field parts from the resolved client values, input parts
// from the call's input record. False when any field/input part has no value
// (the caller omits the position rather than emitting a half-rendered one).
func resolveTemplate(parts []TemplatePart, values map[string]any, record map[string]any) (string, bool) {
	var out strings.Builder
	for _, p := range parts {
		if p.Lit != nil {
			out.WriteString(*p.Lit)
			continue
		}
		if p.Field != nil {
			v, ok := values[strings.Join(p.Field, ".")]
			if !ok || v == nil {
				return "", false
			}
			out.WriteString(formatScalar(v))
			continue
		}
		if p.Input != nil {
			v, ok := record[*p.Input]
			if !ok || v == nil {
				return "", false
			}
			out.WriteString(formatScalar(v))
			continue
		}
		return "", false
	}
	return out.String(), true
}

// resolveValueExpr renders a descriptor value position: a literal verbatim, a
// field reference from the resolved client values, or a template. False when
// the referenced value is absent.
func resolveValueExpr(e ValueExpr, values map[string]any, record map[string]any) (string, bool) {
	if e.Field != nil {
		v, ok := values[strings.Join(e.Field, ".")]
		if !ok || v == nil {
			return "", false
		}
		return formatScalar(v), true
	}
	if e.Template != nil {
		return resolveTemplate(e.Template, values, record)
	}
	if e.Lit != nil {
		return formatScalar(e.Lit), true
	}
	return "", false
}

// declaredHeaders resolves the operation's declared request headers. A header
// whose key or value cannot resolve is omitted whole: half a header is worse
// than none, and the declared chain's absence rules already ran in generated
// code.
func declaredHeaders(d *WireDescriptor, values map[string]any, record map[string]any) map[string]string {
	if len(d.RequestHeaders) == 0 {
		return nil
	}
	headers := make(map[string]string, len(d.RequestHeaders))
	for _, h := range d.RequestHeaders {
		key, ok := resolveTemplate(h.Key, values, record)
		if !ok || key == "" {
			continue
		}
		value, ok := resolveValueExpr(h.Value, values, record)
		if !ok {
			continue
		}
		headers[key] = value
	}
	return headers
}

// buildQuery serializes a query value as a repeated entry per element for a
// list, a single entry otherwise; a null/absent value is omitted (the body's
// nullable-omit rule, applied to the request line). Entries keep binding
// order, matching the other runtimes.
func buildQuery(d *WireDescriptor, record map[string]any) string {
	var entries []string
	add := func(name string, value any) {
		entries = append(entries, url.QueryEscape(name)+"="+url.QueryEscape(formatScalar(value)))
	}
	for _, b := range d.Bindings {
		if b.Part.Kind != "query" {
			continue
		}
		v, ok := record[b.Member]
		if !ok || v == nil {
			continue
		}
		if list, isList := v.([]any); isList {
			for _, element := range list {
				add(b.Part.Name, element)
			}
		} else {
			add(b.Part.Name, v)
		}
	}
	return strings.Join(entries, "&")
}

// buildHeaders layers the request headers: the operation's declared headers
// first, the caller's base headers over them (a bespoke or caller-supplied
// header wins over a declared one), and the input's header bindings last (the
// per-call value is the most specific). A BeforeRequest hook sees the fully
// layered result.
func buildHeaders(d *WireDescriptor, record map[string]any, base map[string]string, values map[string]any) map[string]string {
	headers := declaredHeaders(d, values, record)
	if headers == nil {
		headers = make(map[string]string, len(base))
	}
	for k, v := range base {
		setHeader(headers, k, v)
	}
	for _, b := range d.Bindings {
		if b.Part.Kind != "header" {
			continue
		}
		if v, ok := record[b.Member]; ok && v != nil {
			setHeader(headers, b.Part.Name, formatScalar(v))
		}
	}
	return headers
}

// setHeader overrides across casings: header names are case-insensitive, so a
// bespoke "authorization" must replace a declared "Authorization" rather than
// ride beside it.
func setHeader(headers map[string]string, name, value string) {
	for key := range headers {
		if strings.EqualFold(key, name) {
			delete(headers, key)
		}
	}
	headers[name] = value
}

// buildBody produces the request body: a single payload member as the whole
// body, otherwise the body members assembled into a JSON object in binding
// order (or nil when there are none). A member that is present but null still
// lands in the object as null; only absence omits it.
func buildBody(d *WireDescriptor, record map[string]any) ([]byte, error) {
	var fields []byte
	for _, b := range d.Bindings {
		if b.Part.Kind == "payload" {
			v, ok := record[b.Member]
			if !ok {
				return nil, nil
			}
			return json.Marshal(v)
		}
		if b.Part.Kind != "body" {
			continue
		}
		v, ok := record[b.Member]
		if !ok {
			continue
		}
		// Marshalling a string cannot fail, so the key carries no error path.
		key, _ := json.Marshal(b.Member)
		value, err := json.Marshal(v)
		if err != nil {
			return nil, err
		}
		if fields == nil {
			fields = append(fields, '{')
		} else {
			fields = append(fields, ',')
		}
		fields = append(fields, key...)
		fields = append(fields, ':')
		fields = append(fields, value...)
	}
	if fields == nil {
		return nil, nil
	}
	return append(fields, '}'), nil
}

// hasHeader reports whether a header is already set under any casing: a
// caller-supplied "Content-Type" must suppress the default rather than sit
// beside a second "content-type".
func hasHeader(headers map[string]string, name string) bool {
	for key := range headers {
		if strings.EqualFold(key, name) {
			return true
		}
	}
	return false
}
