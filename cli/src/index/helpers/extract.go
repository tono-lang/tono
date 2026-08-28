// The Go extractor: the exported symbols of one package, read from the
// compiler's export data through go/types, printed as the index's neutral
// JSON. Run by `tono index` from a scratch directory inside the consumer's
// module, so the package resolves the way the generated SDK's imports do.
//
// The export data is what the compiler itself wrote for the package (`go
// list -export` builds it when needed), so what this prints is what the
// compiler sees: no parsing of source, no guessing at build tags. It carries
// no documentation, which the note says.
//
// Standard library only: the helper is compiled on the consumer's machine
// with `go run` and must not pull a dependency into their module.
package main

import (
	"bytes"
	"encoding/json"
	"fmt"
	"go/importer"
	"go/token"
	"go/types"
	"io"
	"os"
	"os/exec"
	"path/filepath"
	"runtime"
	"sort"
	"strings"
)

type member struct {
	Name       string   `json:"name"`
	Kind       string   `json:"kind"`
	Static     bool     `json:"static"`
	Signatures []string `json:"signatures"`
}

type symbol struct {
	Name       string   `json:"name"`
	Kind       string   `json:"kind"`
	Signatures []string `json:"signatures"`
	Doc        string   `json:"doc"`
	Members    []member `json:"members"`
}

type report struct {
	Skipped string   `json:"skipped,omitempty"`
	Note    string   `json:"note,omitempty"`
	Symbols []symbol `json:"symbols"`
}

func emit(r report) {
	if r.Symbols == nil {
		r.Symbols = []symbol{}
	}
	json.NewEncoder(os.Stdout).Encode(r)
}

func skip(format string, args ...interface{}) {
	emit(report{Skipped: fmt.Sprintf(format, args...)})
	os.Exit(0)
}

// The export data file of the package and of every package it depends on:
// the importer needs the dependencies to read the types the package's
// signatures mention.
func exportFiles(importPath string) (map[string]string, error) {
	// The toolchain that compiled this helper is the one whose export data
	// the importer can read: its own `go`, at a fixed path, never the first
	// `go` on PATH.
	goBin := filepath.Join(runtime.GOROOT(), "bin", "go")
	cmd := exec.Command(goBin, "list", "-export", "-deps", "-e", "-f", "{{.ImportPath}}\t{{.Export}}\t{{.Error}}", importPath)
	var stderr bytes.Buffer
	cmd.Stderr = &stderr
	out, err := cmd.Output()
	if err != nil {
		msg := strings.TrimSpace(stderr.String())
		if msg == "" {
			msg = err.Error()
		}
		return nil, fmt.Errorf("go list: %s", lastLine(msg))
	}
	files := map[string]string{}
	for _, line := range strings.Split(strings.TrimSpace(string(out)), "\n") {
		parts := strings.SplitN(line, "\t", 3)
		if len(parts) < 2 {
			continue
		}
		if parts[0] == importPath && len(parts) == 3 && parts[2] != "<nil>" && parts[2] != "" {
			return nil, fmt.Errorf("%s", parts[2])
		}
		files[parts[0]] = parts[1]
	}
	return files, nil
}

func lastLine(s string) string {
	lines := strings.Split(strings.TrimSpace(s), "\n")
	return strings.TrimSpace(lines[len(lines)-1])
}

func main() {
	if len(os.Args) != 2 {
		fmt.Fprintln(os.Stderr, "usage: extract <import path>")
		os.Exit(2)
	}
	importPath := os.Args[1]
	files, err := exportFiles(importPath)
	if err != nil {
		skip("%s", err)
	}
	if files[importPath] == "" {
		skip("package %s has no export data (is it a dependency of the Go module?)", importPath)
	}
	fset := token.NewFileSet()
	imp := importer.ForCompiler(fset, "gc", func(path string) (io.ReadCloser, error) {
		file := files[path]
		if file == "" {
			return nil, fmt.Errorf("no export data for %s", path)
		}
		return os.Open(file)
	})
	pkg, err := imp.Import(importPath)
	if err != nil {
		skip("reading %s: %s", importPath, err)
	}
	emit(report{
		Note:    "Go export data carries no documentation",
		Symbols: symbolsOf(pkg),
	})
}

func symbolsOf(pkg *types.Package) []symbol {
	q := types.RelativeTo(pkg)
	str := func(t types.Type) string { return types.TypeString(t, q) }
	var syms []symbol
	scope := pkg.Scope()
	names := scope.Names()
	sort.Strings(names)
	for _, name := range names {
		obj := scope.Lookup(name)
		if !obj.Exported() {
			continue
		}
		s := symbol{Name: name, Signatures: []string{}, Members: []member{}}
		switch o := obj.(type) {
		case *types.Func:
			s.Kind = "function"
			s.Signatures = []string{str(o.Type())}
		case *types.Const:
			s.Kind = "const"
			s.Signatures = []string{str(o.Type()) + " = " + o.Val().String()}
		case *types.Var:
			s.Kind = "const"
			s.Signatures = []string{str(o.Type())}
		case *types.TypeName:
			s.Kind = "type"
			named, ok := o.Type().(*types.Named)
			if !ok {
				break
			}
			switch u := named.Underlying().(type) {
			case *types.Struct:
				s.Kind = "struct"
				for i := 0; i < u.NumFields(); i++ {
					f := u.Field(i)
					if f.Exported() {
						s.Members = append(s.Members, member{f.Name(), "field", false, []string{str(f.Type())}})
					}
				}
			case *types.Interface:
				s.Kind = "interface"
				for i := 0; i < u.NumMethods(); i++ {
					m := u.Method(i)
					if m.Exported() {
						s.Members = append(s.Members, member{m.Name(), "method", false, []string{str(m.Type())}})
					}
				}
			}
			if tp := named.TypeParams(); tp != nil && tp.Len() > 0 {
				s.Signatures = []string{str(named)}
			}
			for i := 0; i < named.NumMethods(); i++ {
				m := named.Method(i)
				if m.Exported() {
					s.Members = append(s.Members, member{m.Name(), "method", false, []string{str(m.Type())}})
				}
			}
		default:
			continue
		}
		syms = append(syms, s)
	}
	return syms
}
