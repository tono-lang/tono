// The Go spelling of the bespoke bearer-auth recipe: the client_init hook the
// generated SDK binds. Construction resolves the entry's declared fields
// (API_TOKEN from the environment) into the Settings; this hook runs on top
// (bespoke wins) and writes the Authorization header into the transport state,
// so every request carries it with no per-call plumbing and no environment
// read of its own.
//
// This is a recipe to copy, not a shipped scheme: there is no built-in auth.
//
// Go calls a bound symbol unqualified, from inside the generated package, so
// this file is dropped next to the generated sources.

package auth

import "errors"

func ApplyBearer(s *Settings) error {
	if s.APIToken == "" {
		return errors.New("API_TOKEN is not set")
	}
	s.Headers["authorization"] = "Bearer " + s.APIToken
	return nil
}
