"""Ground-truth helper for fastapi-dead-deprecated.

Lists functions/methods defined in <pkg> that have no Name/Attribute/import
reference (string mentions do not count) inside <pkg>, excluding
self-recursion, with the number of references elsewhere under <root>.

    python3 -I py_unreferenced.py <repoRoot> fastapi
"""
import ast, os, sys, collections
root, pkg = sys.argv[1], sys.argv[2]
ref_pkg = collections.Counter(); ref_other = collections.Counter()
defs = []
class R(ast.NodeVisitor):
    def __init__(self, counter, collect, path):
        self.c, self.collect, self.path, self.stack = counter, collect, path, []
    def _scope(self, node):
        if self.collect and isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef)) and len(self.stack) <= 1:
            parent = self.stack[-1] if self.stack else None
            if parent is None or parent[0] == 'class':
                defs.append((self.path, node.lineno, node.name, parent[1] if parent else None))
        self.stack.append(('class' if isinstance(node, ast.ClassDef) else 'func', node.name))
        self.generic_visit(node); self.stack.pop()
    visit_FunctionDef = visit_AsyncFunctionDef = visit_ClassDef = _scope
    def _cur_func(self):
        names = [n for k, n in self.stack if k == 'func']
        return names[0] if names else None
    def visit_Name(self, node):
        if node.id != self._cur_func(): self.c[node.id] += 1
    def visit_Attribute(self, node):
        if node.attr != self._cur_func(): self.c[node.attr] += 1
        self.generic_visit(node)
    def visit_ImportFrom(self, node):
        for a in node.names: self.c[a.name] += 1
for dp, dn, fn in os.walk(root):
    dn[:] = [d for d in dn if d not in ('.git', 'node_modules', '__pycache__')]
    for f in fn:
        if not f.endswith('.py'): continue
        p = os.path.join(dp, f)
        inpkg = p.startswith(os.path.join(root, pkg) + os.sep)
        try: tree = ast.parse(open(p, encoding='utf-8').read())
        except SyntaxError: continue
        R(ref_pkg if inpkg else ref_other, inpkg, p).visit(tree)
for p, line, name, cls in sorted(defs):
    if name.startswith('__') and name.endswith('__'): continue
    if ref_pkg[name] == 0:
        print(f'{p}:{line}\t{(cls + ".") if cls else ""}{name}\tpkg_refs=0\tother_refs={ref_other[name]}')
