// Ground-truth helper for the Vue `vue-cfg-app-config-files` task (#163):
// every access to a member of the `AppConfig` interface (what `app.config`
// holds) in packages/*/src, found by the TypeScript type checker, not by name.
// A property access `x.member` counts when the checker types `x` as
// `AppConfig`; so does each element of an object destructuring pattern typed
// as `AppConfig` (`const { errorHandler } = instance.appContext.config`).
// Needs the `typescript` package: set TS_PATH to its directory.
//
//   node vue_config.js <repoDir> > app-config.tsv
//
// One line per access:  member <TAB> file:line <TAB> read|write|destructure
const ts = require(process.env.TS_PATH || 'typescript');
const path = require('path'), fs = require('fs');
const root = path.resolve(process.argv[2]);
const iface = 'AppConfig';
const pkgs = path.join(root, 'packages');
function walk(d, out) {
  if (!fs.existsSync(d)) return out;
  for (const e of fs.readdirSync(d, { withFileTypes: true })) {
    const p = path.join(d, e.name);
    if (e.isDirectory()) walk(p, out);
    else if (/\.tsx?$/.test(e.name) && !e.name.endsWith('.d.ts')) out.push(p);
  }
  return out;
}
const files = [];
for (const p of fs.readdirSync(pkgs).sort()) walk(path.join(pkgs, p, 'src'), files);
files.push(path.join(pkgs, 'global.d.ts'));
const options = {
  target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.ESNext, moduleResolution: ts.ModuleResolutionKind.Bundler,
  strict: true, skipLibCheck: true, noEmit: true, types: [], lib: ['lib.es2022.d.ts', 'lib.dom.d.ts'], baseUrl: root,
  paths: { '@vue/compat': [path.join(pkgs, 'vue-compat/src')], '@vue/*': [path.join(pkgs, '*/src')], 'vue': [path.join(pkgs, 'vue/src')] },
};
const program = ts.createProgram(files, options);
const checker = program.getTypeChecker();
function isIface(t) {
  if (!t) return false;
  if (t.isUnion && t.isUnion()) return t.types.some(isIface);
  const s = t.getSymbol && t.getSymbol();
  return !!(s && s.name === iface);
}
const out = [];
for (const sf of program.getSourceFiles()) {
  if (!sf.fileName.startsWith(pkgs + path.sep) || sf.isDeclarationFile) continue;
  const rel = path.relative(root, sf.fileName);
  const line = n => sf.getLineAndCharacterOfPosition(n.getStart(sf)).line + 1;
  const visit = n => {
    if (ts.isPropertyAccessExpression(n) && isIface(checker.getTypeAtLocation(n.expression))) {
      const write = ts.isBinaryExpression(n.parent) && n.parent.left === n &&
        n.parent.operatorToken.kind === ts.SyntaxKind.EqualsToken;
      out.push([n.name.text, `${rel}:${line(n)}`, write ? 'write' : 'read'].join('\t'));
    }
    if (ts.isObjectBindingPattern(n) && isIface(checker.getTypeAtLocation(n))) {
      for (const el of n.elements) out.push([(el.propertyName || el.name).getText(sf), `${rel}:${line(el)}`, 'destructure'].join('\t'));
    }
    ts.forEachChild(n, visit);
  };
  visit(sf);
}
console.log(out.sort().join('\n'));
