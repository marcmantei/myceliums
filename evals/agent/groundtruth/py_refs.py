"""Ground-truth helper for the Python (fastapi) callers/impact tasks.

Lists call sites (and with --refs, every Name/Attribute/import reference) of a
name in the .py files under <root>, with the enclosing qualified name.

    python3 -I py_refs.py <root> <name> [--refs]
"""
import ast, os, sys

def iter_files(root):
    for dp, dn, fn in os.walk(root):
        dn[:] = [d for d in dn if d not in ('.git', 'node_modules', '__pycache__')]
        for f in fn:
            if f.endswith('.py'):
                yield os.path.join(dp, f)

class V(ast.NodeVisitor):
    def __init__(self, name, path, refs):
        self.name, self.path, self.refs = name, path, refs
        self.stack = []
        self.out = []
    def _scope(self, node):
        self.stack.append(node.name)
        self.generic_visit(node)
        self.stack.pop()
    visit_FunctionDef = visit_AsyncFunctionDef = visit_ClassDef = _scope
    def where(self):
        return '.'.join(self.stack) or '<module>'
    def visit_Call(self, node):
        f = node.func
        nm = f.id if isinstance(f, ast.Name) else f.attr if isinstance(f, ast.Attribute) else None
        if nm == self.name:
            self.out.append((self.path, node.lineno, self.where(), 'call', ast.unparse(f)))
        self.generic_visit(node)
    def visit_Name(self, node):
        if self.refs and node.id == self.name and isinstance(node.ctx, ast.Load):
            self.out.append((self.path, node.lineno, self.where(), 'name', node.id))
    def visit_Attribute(self, node):
        if self.refs and node.attr == self.name:
            self.out.append((self.path, node.lineno, self.where(), 'attr', ast.unparse(node)))
        self.generic_visit(node)
    def visit_ImportFrom(self, node):
        for a in node.names:
            if a.name == self.name:
                self.out.append((self.path, node.lineno, self.where(), 'import', f'from {node.module} import {a.name}'))

def main():
    root, name = sys.argv[1], sys.argv[2]
    refs = '--refs' in sys.argv
    res = []
    for p in sorted(iter_files(root)):
        try:
            tree = ast.parse(open(p, encoding='utf-8').read(), p)
        except SyntaxError as e:
            print('SYNTAX', p, e, file=sys.stderr); continue
        v = V(name, p, refs); v.visit(tree); res += v.out
    seen = set()
    for r in res:
        if r in seen: continue
        seen.add(r)
        print(f'{r[0]}:{r[1]}\t{r[2]}\t{r[3]}\t{r[4]}')
main()
