// The Go driver of the FFI bench: constructs the generated client against the
// real stand-in library (no stub anywhere) and checks the values that come
// back through both operations. The fallback answers its first calculator,
// the constant one, so combined_value is the base; the series answers its
// last value.
package main

import (
	"context"
	"fmt"
	"os"

	"example.com/mathkit/mathkit"
)

func main() {
	c, err := mathkit.New(1.5, "2 * 3", 2, []float64{1, 2, 3})
	if err != nil {
		fail("construct: %v", err)
	}
	combined, err := c.CombinedValue(context.Background())
	if err != nil {
		fail("combined_value: %v", err)
	}
	if combined != 1.5 {
		fail("combined_value: want 1.5, got %v", combined)
	}
	series, err := c.SeriesValue(context.Background())
	if err != nil {
		fail("series_value: %v", err)
	}
	if series != 3 {
		fail("series_value: want 3, got %v", series)
	}
	fmt.Println("ffi bench (go): ok")
}

func fail(format string, args ...any) {
	fmt.Fprintf(os.Stderr, format+"\n", args...)
	os.Exit(1)
}
