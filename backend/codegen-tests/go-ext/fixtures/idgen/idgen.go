// Package idgen is a stand-in for a real single-return third-party library
// (github.com/google/uuid's NewString, which returns only a string, no
// error): the repro for the "extern infallible" Go emission fix.
package idgen

func NewString() string {
	return "00000000-0000-0000-0000-000000000000"
}
