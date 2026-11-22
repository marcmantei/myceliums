// Ground-truth helper for the Vue `vue-cfg-ast-node-interfaces` task (#163):
// the interfaces and classes in packages/*/src whose heritage clauses reach a
// given declaration, resolved by the TypeScript type checker (so the template
// compiler's AST `Node` is never confused with the DOM's `Node`).
// Needs the `typescript` package: set TS_PATH to its directory.
//
//   node vue_hierarchy.js <repoDir> <file> <name> > ast-node.tsv
//
// One line per descendant:  file#name <TAB> line <TAB> direct|indirect
const ts = require(process.env.TS_PATH || "typescript");
const path = require('path'), fs = require('fs');
const root = path.resolve(process.argv[2]); const baseFile = process.argv[3]; const baseName = process.argv[4];
const pkgs = path.join(root, 'packages');
function walk(d, out) { if (!fs.existsSync(d)) return out; for (const e of fs.readdirSync(d, { withFileTypes: true })) { const p = path.join(d, e.name); if (e.isDirectory()) walk(p, out); else if (/\.tsx?$/.test(e.name) && !e.name.endsWith('.d.ts')) out.push(p); } return out; }
const files = []; for (const p of fs.readdirSync(pkgs).sort()) walk(path.join(pkgs, p, 'src'), files); files.push(path.join(pkgs, 'global.d.ts'));
const options = { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.ESNext, moduleResolution: ts.ModuleResolutionKind.Bundler, strict: true, skipLibCheck: true, noEmit: true, types: [], lib: ['lib.es2022.d.ts','lib.dom.d.ts'], baseUrl: root,
  paths: { '@vue/compat': [path.join(pkgs,'vue-compat/src')], '@vue/*': [path.join(pkgs,'*/src')], 'vue': [path.join(pkgs,'vue/src')] } };
const program = ts.createProgram(files, options); const checker = program.getTypeChecker();
const decls = new Map(); // key file#name -> {node, parents:[keys]}
const key = d => path.relative(root, d.getSourceFile().fileName) + '#' + d.name.text;
for (const sf of program.getSourceFiles()) {
  if (!sf.fileName.startsWith(pkgs + path.sep) || sf.isDeclarationFile) continue;
  ts.forEachChild(sf, function v(n) {
    if ((ts.isInterfaceDeclaration(n) || ts.isClassDeclaration(n)) && n.name) {
      const parents = [];
      for (const h of n.heritageClauses || []) for (const t of h.types) {
        let s = checker.getSymbolAtLocation(t.expression); if (!s) continue;
        if (s.flags & ts.SymbolFlags.Alias) { try { s = checker.getAliasedSymbol(s); } catch (e) {} }
        for (const d of s.declarations || []) if ((ts.isInterfaceDeclaration(d) || ts.isClassDeclaration(d)) && d.name && d.getSourceFile().fileName.startsWith(pkgs)) parents.push(key(d));
      }
      decls.set(key(n), { parents, line: sf.getLineAndCharacterOfPosition(n.getStart()).line + 1 });
    }
    ts.forEachChild(n, v);
  });
}
const target = baseFile + '#' + baseName
function reaches(k, seen = new Set()) { if (k === target) return true; if (seen.has(k)) return false; seen.add(k); const d = decls.get(k); return !!d && d.parents.some(p => reaches(p, seen)); }
const out = [];
for (const [k, d] of decls) if (k !== target && reaches(k)) out.push(k + "\t" + d.line + "\t" + (d.parents.includes(target) ? "direct" : "indirect"));
console.log(out.sort().join("\n"));
