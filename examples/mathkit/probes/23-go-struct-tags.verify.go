// Runs the generated SDK against the stand-in library for real: the
// calibration is resolved from the environment by reflection over the `env`
// tags of the generated struct, with {profile} substituted at run time, and
// the pinned defaults answer when the variables are absent. A wrong or
// missing tag compiles; only this run turns it red.
package main

import (
	"context"
	"fmt"
	"os"

	mathkit "example.com/mathkit/mathkit"
)

func main() {
	os.Setenv("CALC_alpha_SCALE", "2.5")
	os.Setenv("CALC_alpha_OFFSET", "0.5")
	ctx := context.Background()
	defaults := mathkit.Calibration{Scale: 1, Offset: 0}

	tuned, err := mathkit.New("alpha", defaults)
	if err != nil {
		fmt.Fprintln(os.Stderr, "construction failed:", err)
		os.Exit(1)
	}
	got, err := tuned.Calibrate(ctx)
	if err != nil {
		fmt.Fprintln(os.Stderr, "calibrate failed:", err)
		os.Exit(1)
	}
	if want := (mathkit.Calibration{Scale: 2.5, Offset: 0.5}); got != want {
		fmt.Fprintf(os.Stderr, "calibrate: got %+v, want %+v\n", got, want)
		os.Exit(1)
	}

	// No variable carries the beta profile: the fallback answers the pinned
	// defaults instead of failing.
	fallback, err := mathkit.New("beta", defaults)
	if err != nil {
		fmt.Fprintln(os.Stderr, "construction failed:", err)
		os.Exit(1)
	}
	got, err = fallback.Calibrate(ctx)
	if err != nil {
		fmt.Fprintln(os.Stderr, "fallback calibrate failed:", err)
		os.Exit(1)
	}
	if got != defaults {
		fmt.Fprintf(os.Stderr, "fallback calibrate: got %+v, want %+v\n", got, defaults)
		os.Exit(1)
	}
	fmt.Println("ok")
}
