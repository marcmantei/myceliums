// Command go_calls cross-checks the Go gold set against go/types.
//
//		cd crates/myceliums-benchmarks/graph-gold/verify/go_calls && go run .
//
//	 1. Symbols: every func, method, named type and interface method must be in
//	    the gold set at the same line, and nothing else.
//	 2. Calls: every call expression is resolved by the type checker; a call to
//	    a fixture function or method must be a gold edge (primary or
//	    also_accept), and every gold edge must be seen. Function literals belong
//	    to the enclosing declaration.
//
// Exits non-zero on any disagreement.
package main

import (
	"encoding/json"
	"fmt"
	"go/ast"
	"go/build"
	"go/importer"
	"go/parser"
	"go/token"
	"go/types"
	"os"
	"path/filepath"
	"runtime"
	"sort"
	"strings"
)

type gold struct {
	Symbols []struct {
		ID   string `json:"id"`
		Line int    `json:"line"`
	} `json:"symbols"`
	Calls []struct {
		From       string   `json:"from"`
		To         string   `json:"to"`
		AlsoAccept []string `json:"also_accept"`
	} `json:"calls"`
	Dynamic []struct {
		From string `json:"from"`
		To   string `json:"to"`
	} `json:"dynamic"`
}

const module = "example.com/shop"

func main() {
	_, self, _, _ := runtime.Caller(0)
	goldDir := filepath.Dir(filepath.Dir(filepath.Dir(self)))
	root := filepath.Join(goldDir, "..", "..", "..", "tests", "fixtures", "graph-accuracy", "go")
	root, err := filepath.Abs(root)
	check(err)
	var g gold
	data, err := os.ReadFile(filepath.Join(goldDir, "go.json"))
	check(err)
	check(json.Unmarshal(data, &g))

	fset := token.NewFileSet()
	found := map[string]int{}      // gold-style id -> line
	keyToID := map[string]string{} // pkgpath.Recv.Name -> id
	type pkgFiles struct {
		path  string
		files []*ast.File
	}
	var pkgs []pkgFiles
	check(filepath.Walk(root, func(p string, info os.FileInfo, err error) error {
		check(err)
		if !info.IsDir() {
			return nil
		}
		parsed, err := parser.ParseDir(fset, p, nil, parser.ParseComments)
		check(err)
		for _, pkg := range parsed {
			rel, _ := filepath.Rel(root, p)
			path := module
			if rel != "." {
				path = module + "/" + filepath.ToSlash(rel)
			}
			var files []*ast.File
			for _, f := range pkg.Files {
				files = append(files, f)
			}
			pkgs = append(pkgs, pkgFiles{path, files})
		}
		return nil
	}))

	for _, pkg := range pkgs {
		for _, file := range pkg.files {
			name := fset.Position(file.Pos()).Filename
			rel, _ := filepath.Rel(root, name)
			rel = filepath.ToSlash(rel)
			for _, decl := range file.Decls {
				switch d := decl.(type) {
				case *ast.FuncDecl:
					qual := d.Name.Name
					if d.Recv != nil {
						qual = recvName(d.Recv.List[0].Type) + "." + qual
					}
					id := rel + "::" + qual
					found[id] = fset.Position(d.Name.Pos()).Line
					keyToID[pkg.path+"."+qual] = id
				case *ast.GenDecl:
					for _, spec := range d.Specs {
						ts, ok := spec.(*ast.TypeSpec)
						if !ok {
							continue
						}
						id := rel + "::" + ts.Name.Name
						found[id] = fset.Position(ts.Name.Pos()).Line
						if it, ok := ts.Type.(*ast.InterfaceType); ok {
							for _, m := range it.Methods.List {
								for _, n := range m.Names {
									qual := ts.Name.Name + "." + n.Name
									found[rel+"::"+qual] = fset.Position(n.Pos()).Line
									keyToID[pkg.path+"."+qual] = rel + "::" + qual
								}
							}
						}
					}
				}
			}
		}
	}

	var failures []string
	goldLines := map[string]int{}
	for _, s := range g.Symbols {
		goldLines[s.ID] = s.Line
	}
	for id, line := range found {
		if gl, ok := goldLines[id]; !ok {
			failures = append(failures, fmt.Sprintf("symbol missing from gold: %s (line %d)", id, line))
		} else if gl != line {
			failures = append(failures, fmt.Sprintf("symbol line differs: %s gold %d go/types %d", id, gl, line))
		}
	}
	for id := range goldLines {
		if _, ok := found[id]; !ok {
			failures = append(failures, "gold symbol not in source: "+id)
		}
	}

	observed := map[string]bool{}
	// The source importer resolves module paths from build.Default.Dir.
	build.Default.Dir = root
	imp := importer.ForCompiler(fset, "source", nil)
	for _, pkg := range pkgs {
		info := &types.Info{Uses: map[*ast.Ident]types.Object{}, Selections: map[*ast.SelectorExpr]*types.Selection{}}
		conf := types.Config{Importer: imp}
		_, err := conf.Check(pkg.path, fset, pkg.files, info)
		check(err)
		for _, file := range pkg.files {
			for _, decl := range file.Decls {
				fd, ok := decl.(*ast.FuncDecl)
				if !ok || fd.Body == nil {
					continue
				}
				qual := fd.Name.Name
				if fd.Recv != nil {
					qual = recvName(fd.Recv.List[0].Type) + "." + qual
				}
				from := keyToID[pkg.path+"."+qual]
				ast.Inspect(fd.Body, func(n ast.Node) bool {
					call, ok := n.(*ast.CallExpr)
					if !ok {
						return true
					}
					if fn := callee(call.Fun, info); fn != nil && fn.Pkg() != nil {
						key := fn.Pkg().Path() + "."
						if sig, ok := fn.Type().(*types.Signature); ok && sig.Recv() != nil {
							key += typeName(sig.Recv().Type()) + "."
						}
						if to, ok := keyToID[key+fn.Name()]; ok {
							observed[from+" -> "+to] = true
						}
					}
					return true
				})
			}
		}
	}

	accepted := map[string]int{}
	for i, c := range g.Calls {
		for _, t := range append([]string{c.To}, c.AlsoAccept...) {
			accepted[c.From+" -> "+t] = i
		}
	}
	dynamic := map[string]bool{}
	for _, d := range g.Dynamic {
		dynamic[d.From+" -> "+d.To] = true
	}
	seen := map[int]bool{}
	var pairs []string
	for p := range observed {
		pairs = append(pairs, p)
	}
	sort.Strings(pairs)
	for _, p := range pairs {
		if dynamic[p] {
			continue
		}
		if i, ok := accepted[p]; ok {
			seen[i] = true
		} else {
			failures = append(failures, "resolved call not in gold: "+p)
		}
	}
	for i, c := range g.Calls {
		if !seen[i] {
			failures = append(failures, "gold edge never resolved: "+c.From+" -> "+c.To)
		}
	}
	sort.Strings(failures)
	for _, f := range failures {
		fmt.Println(f)
	}
	fmt.Printf("%d symbols, %d gold edges, %d disagreements\n", len(found), len(g.Calls), len(failures))
	if len(failures) > 0 {
		os.Exit(1)
	}
}

// callee is the function or method a call expression invokes, if static.
func callee(fun ast.Expr, info *types.Info) *types.Func {
	switch f := fun.(type) {
	case *ast.ParenExpr:
		return callee(f.X, info)
	case *ast.IndexExpr: // explicit generic instantiation
		return callee(f.X, info)
	case *ast.IndexListExpr:
		return callee(f.X, info)
	case *ast.Ident:
		fn, _ := info.Uses[f].(*types.Func)
		return fn
	case *ast.SelectorExpr:
		if sel, ok := info.Selections[f]; ok {
			fn, _ := sel.Obj().(*types.Func)
			return fn
		}
		fn, _ := info.Uses[f.Sel].(*types.Func)
		return fn
	}
	return nil
}

// recvName is the base type name of a receiver expression.
func recvName(e ast.Expr) string {
	switch t := e.(type) {
	case *ast.StarExpr:
		return recvName(t.X)
	case *ast.IndexExpr:
		return recvName(t.X)
	case *ast.IndexListExpr:
		return recvName(t.X)
	case *ast.Ident:
		return t.Name
	}
	panic(fmt.Sprintf("unexpected receiver %T", e))
}

// typeName is the base named type of a receiver type.
func typeName(t types.Type) string {
	if p, ok := t.(*types.Pointer); ok {
		t = p.Elem()
	}
	if n, ok := t.(*types.Named); ok {
		return n.Obj().Name()
	}
	return strings.TrimPrefix(t.String(), "*")
}

func check(err error) {
	if err != nil {
		fmt.Fprintln(os.Stderr, err)
		os.Exit(2)
	}
}
