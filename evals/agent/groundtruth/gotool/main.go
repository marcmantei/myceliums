// gotool: ground-truth helper for the Go (fzf) tasks, built on go/packages,
// go/types and golang.org/x/tools call graphs.
//
//	gotool <moduleDir> callers <funcSuffix>   // e.g. "src.Terminal).executeCommand"; CHA=1 for the CHA graph
//	gotool <moduleDir> callees <funcSuffix>
//	gotool <moduleDir> refs <objName>         // identifier uses by file:line and enclosing function
//	gotool <moduleDir> unused                  // funcs/methods with zero uses (ALL=1: exported too)
//	gotool <moduleDir> edges                   // every call edge between functions of the module:
//	                                           // caller<TAB>callee<TAB>file:line<TAB>static|iface
//
// WITH_TESTS=1 loads test packages too; set GOOS to check other platforms' files.
package main

import (
	"fmt"
	"go/ast"
	"go/token"
	"go/types"
	"os"
	"path/filepath"
	"sort"
	"strings"

	"golang.org/x/tools/go/callgraph"
	"golang.org/x/tools/go/callgraph/cha"
	"golang.org/x/tools/go/callgraph/static"
	"golang.org/x/tools/go/packages"
	"golang.org/x/tools/go/ssa"
	"golang.org/x/tools/go/ssa/ssautil"
)

func main() {
	dir, cmd := os.Args[1], os.Args[2]
	arg := ""
	if len(os.Args) > 3 {
		arg = os.Args[3]
	}
	cfg := &packages.Config{Mode: packages.LoadAllSyntax, Dir: dir, Tests: os.Getenv("WITH_TESTS") == "1"}
	pkgs, err := packages.Load(cfg, "./...")
	if err != nil {
		panic(err)
	}
	if packages.PrintErrors(pkgs) > 0 {
		fmt.Fprintln(os.Stderr, "(package errors above)")
	}
	abs, _ := filepath.Abs(dir)
	rel := func(p string) string { r, _ := filepath.Rel(abs, p); return r }
	switch cmd {
	case "callers", "callees":
		prog, _ := ssautil.AllPackages(pkgs, ssa.InstantiateGenerics)
		prog.Build()
		var cg *callgraph.Graph
		if os.Getenv("CHA") == "1" {
			cg = cha.CallGraph(prog)
		} else {
			cg = static.CallGraph(prog)
		}
		cg.DeleteSyntheticNodes()
		out := map[string]bool{}
		for fn, node := range cg.Nodes {
			if fn == nil || !strings.HasSuffix(fn.String(), arg) {
				continue
			}
			fmt.Println("TARGET", fn.String())
			edges := node.In
			if cmd == "callees" {
				edges = node.Out
			}
			for _, e := range edges {
				other := e.Caller.Func
				if cmd == "callees" {
					other = e.Callee.Func
				}
				pos := prog.Fset.Position(e.Pos())
				out[fmt.Sprintf("%s\t%s:%d", other.String(), rel(pos.Filename), pos.Line)] = true
			}
		}
		keys := make([]string, 0, len(out))
		for k := range out {
			keys = append(keys, k)
		}
		sort.Strings(keys)
		for _, k := range keys {
			fmt.Println(k)
		}
	case "edges":
		// Every call edge between functions declared in the module's own
		// files, for transitive questions (#125). Unlike static.CallGraph,
		// which only follows static calls from package members, this visits
		// every function (closures included) and labels each edge: "static"
		// for a call with a static callee, "iface" for an interface method
		// call resolved to each implementation (class-hierarchy analysis).
		// Calls through function values are not resolved.
		prog, _ := ssautil.AllPackages(pkgs, ssa.InstantiateGenerics)
		prog.Build()
		chaGraph := cha.CallGraph(prog)
		inModule := func(fn *ssa.Function) bool {
			if fn == nil {
				return false
			}
			f := prog.Fset.Position(fn.Pos()).Filename
			return f != "" && strings.HasPrefix(f, abs+string(filepath.Separator))
		}
		out := map[string]bool{}
		for fn := range ssautil.AllFunctions(prog) {
			if !inModule(fn) {
				continue
			}
			for _, b := range fn.Blocks {
				for _, instr := range b.Instrs {
					site, ok := instr.(ssa.CallInstruction)
					if !ok {
						continue
					}
					pos := prog.Fset.Position(site.Pos())
					emit := func(callee *ssa.Function, kind string) {
						if inModule(callee) {
							out[fmt.Sprintf("%s\t%s\t%s:%d\t%s", fn.String(), callee.String(), rel(pos.Filename), pos.Line, kind)] = true
						}
					}
					if g := site.Common().StaticCallee(); g != nil {
						emit(g, "static")
					} else if site.Common().IsInvoke() {
						if node := chaGraph.Nodes[fn]; node != nil {
							for _, e := range node.Out {
								if e.Site == site {
									emit(e.Callee.Func, "iface")
								}
							}
						}
					}
				}
			}
		}
		keys := make([]string, 0, len(out))
		for k := range out {
			keys = append(keys, k)
		}
		sort.Strings(keys)
		for _, k := range keys {
			fmt.Println(k)
		}
	case "refs":
		for _, p := range pkgs {
			for id, obj := range p.TypesInfo.Uses {
				if obj == nil || obj.Name() != arg {
					continue
				}
				pos := p.Fset.Position(id.Pos())
				fmt.Printf("%s:%d\t%s\t%s\n", rel(pos.Filename), pos.Line, enclosing(p, id.Pos()), types.ObjectString(obj, nil))
			}
		}
	case "unused":
		// Key objects by declaration position so package variants (with/without tests) merge.
		key := func(p *packages.Package, obj types.Object) string { return p.Fset.Position(obj.Pos()).String() }
		uses := map[string]int{}
		for _, p := range pkgs {
			for _, obj := range p.TypesInfo.Uses {
				if obj != nil {
					uses[key(p, obj)]++
					if f, ok := obj.(*types.Func); ok && f.Origin() != f {
						uses[key(p, f.Origin())]++
					}
				}
			}
			for _, sel := range p.TypesInfo.Selections {
				uses[key(p, sel.Obj())]++
			}
		}
		for _, p := range pkgs {
			for id, obj := range p.TypesInfo.Defs {
				f, ok := obj.(*types.Func)
				if !ok || (f.Exported() && os.Getenv("ALL") != "1") || f.Name() == "main" || f.Name() == "init" || f.Name() == "_" {
					continue
				}
				if uses[key(p, f)] == 0 {
					pos := p.Fset.Position(id.Pos())
					fmt.Printf("%s:%d\t%s\n", rel(pos.Filename), pos.Line, types.ObjectString(f, nil))
				}
			}
		}
	}
}

func enclosing(p *packages.Package, pos token.Pos) string {
	for _, f := range p.Syntax {
		if f.Pos() <= pos && pos <= f.End() {
			name := "<file>"
			ast.Inspect(f, func(n ast.Node) bool {
				if fd, ok := n.(*ast.FuncDecl); ok && fd.Pos() <= pos && pos <= fd.End() {
					name = fd.Name.Name
					if fd.Recv != nil && len(fd.Recv.List) > 0 {
						name = types.ExprString(fd.Recv.List[0].Type) + "." + name
					}
				}
				return true
			})
			return name
		}
	}
	return "?"
}
