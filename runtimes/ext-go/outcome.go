// Package tonoext carries the types a bespoke operation implementation
// exchanges with the generated glue. It is deliberately data only: it performs
// no I/O, imposes no transport, and has no dependency on the HTTP runtime, so
// an operation implemented over a legacy library, a cache, or a crypto module
// pulls in nothing it does not use.
package tonoext

// Outcome is what a raw-form implementation returns instead of the operation's
// declared output. The generated glue treats it exactly as it treats a protocol
// response: on Success the Body is decoded strictly into the declared output
// type, otherwise Code is matched against the operation's declared error codes
// with a generic fallback. Returning it costs the implementation no mapping
// code when the shapes already line up.
//
// Body is the JSON encoding of the declared output (on success) or of the
// declared error (on failure). Code is read only when Success is false.
type Outcome struct {
	Success bool
	Code    string
	Body    []byte
}
