package tonohttp

import (
	"encoding/json"
	"net/url"
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

// buildPath substitutes each label binding into its {name} placeholder. A path
// parameter must be present, so an absent or null one substitutes empty rather
// than a literal "null".
func buildPath(d *WireDescriptor, record map[string]any) string {
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
	return path
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

func buildHeaders(d *WireDescriptor, record map[string]any, base map[string]string) map[string]string {
	headers := make(map[string]string, len(base))
	for k, v := range base {
		headers[k] = v
	}
	for _, b := range d.Bindings {
		if b.Part.Kind != "header" {
			continue
		}
		if v, ok := record[b.Member]; ok && v != nil {
			headers[b.Part.Name] = formatScalar(v)
		}
	}
	return headers
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
