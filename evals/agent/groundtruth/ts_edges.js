// Ground-truth helper for the hard TypeScript (hono) tasks (#125): every call
// edge of the program, resolved by the TypeScript type checker.
// Needs the `typescript` package: set TS_PATH to its directory.
//
//   node ts_edges.js <repoDir> > edges.tsv
//
// One line per call site:
//   caller <TAB> callee <TAB> file:line <TAB> kind <TAB> callee_file
// `caller` is the outermost named function, method or const-bound function
// that contains the call (`Class.method`, `functionName`, `<module>` for
// top-level code); `callee` is the declaration the checker resolves the call
// to, through import aliases and re-exports. `kind` is `direct` when the
// callee is a function, method, constructor or const-bound function body,
// `iface` when it is a member of an interface or type literal (dynamic
// dispatch, not resolved further) and `value` when it is a parameter, a
// property or a variable holding a function value. Files under src/, tests
// included; `.d.ts` files are skipped.
const ts = require(process.env.TS_PATH || 'typescript');
const path = require('path'), fs = require('fs');
const [repo] = process.argv.slice(2);
const root = path.resolve(repo);
function walk(d, out) {
  for (const e of fs.readdirSync(d, { withFileTypes: true })) {
    if (e.name === 'node_modules' || e.name.startsWith('.')) continue;
    const p = path.join(d, e.name);
    if (e.isDirectory()) walk(p, out);
    else if (/\.(ts|tsx)$/.test(e.name) && !e.name.endsWith('.d.ts')) out.push(p);
  }
  return out;
}
const files = walk(path.join(root, 'src'), []);
const options = { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.ESNext, moduleResolution: ts.ModuleResolutionKind.Bundler,
  strict: true, skipLibCheck: true, jsx: ts.JsxEmit.ReactJSX, jsxImportSource: 'hono/jsx', allowJs: false, noEmit: true,
  paths: { 'hono/jsx/jsx-runtime': [path.join(root, 'src/jsx/jsx-runtime.ts')], 'hono/jsx/jsx-dev-runtime': [path.join(root, 'src/jsx/jsx-dev-runtime.ts')], 'hono/*': [path.join(root, 'src/*')], 'hono': [path.join(root, 'src/index.ts')] }, baseUrl: root };
const program = ts.createProgram(files, options);
const checker = program.getTypeChecker();
const rel = f => path.relative(root, f);

function isFunctionLike(n) {
  return n && (ts.isArrowFunction(n) || ts.isFunctionExpression(n));
}
// Name of a declaration node that defines a function, or null.
function declName(node) {
  const cls = c => (c && (ts.isClassDeclaration(c) || ts.isClassExpression(c)) && c.name) ? c.name.text : null;
  if (ts.isFunctionDeclaration(node) && node.name) return node.name.text;
  if ((ts.isMethodDeclaration(node) || ts.isGetAccessor(node) || ts.isSetAccessor(node)) && node.name) {
    const c = cls(node.parent);
    if (c) return c + '.' + node.name.getText();
    return null; // object-literal method: attributed to the enclosing named function
  }
  if (ts.isConstructorDeclaration(node)) { const c = cls(node.parent); return c ? c + '.constructor' : null; }
  if (ts.isPropertyDeclaration(node) && node.name && isFunctionLike(node.initializer)) {
    const c = cls(node.parent);
    return c ? c + '.' + node.name.getText() : null;
  }
  if (ts.isVariableDeclaration(node) && ts.isIdentifier(node.name) && isFunctionLike(node.initializer)) return node.name.text;
  return null;
}
function enclosing(node) {
  let name = '<module>';
  let inFunction = false;
  for (let n = node.parent; n; n = n.parent) {
    if (ts.isFunctionLike(n)) inFunction = true;
    let d = declName(n);
    // `const upgradeWebSocket = defineWebSocketHelper(async (c) => ...)`:
    // code in a callback of a call that initialises a variable belongs to
    // that variable, as an agent would name it.
    if (!d && inFunction && ts.isVariableDeclaration(n) && ts.isIdentifier(n.name)) d = n.name.text;
    if (d) name = d; // keep walking: the outermost named function wins
  }
  return name;
}
function resolve(expr) {
  let target = expr;
  if (ts.isPropertyAccessExpression(target)) target = target.name;
  else if (ts.isElementAccessExpression(target)) return null;
  let sym = checker.getSymbolAtLocation(target);
  if (!sym) return null;
  if (sym.flags & ts.SymbolFlags.Alias) { try { sym = checker.getAliasedSymbol(sym); } catch (e) { return null; } }
  const decls = sym.declarations || [];
  if (!decls.length) return null;
  const d = decls[decls.length - 1].kind === ts.SyntaxKind.FunctionDeclaration ? decls.find(x => x.body) || decls[0] : decls[0];
  const file = d.getSourceFile().fileName;
  let name = declName(d), kind = 'direct';
  if (!name) {
    if (ts.isMethodSignature(d) || ts.isPropertySignature(d) || ts.isCallSignatureDeclaration(d)) {
      const owner = d.parent && (ts.isInterfaceDeclaration(d.parent) || ts.isTypeAliasDeclaration(d.parent)) && d.parent.name ? d.parent.name.text : '<type>';
      name = owner + '.' + (d.name ? d.name.getText() : '()');
      kind = 'iface';
    } else if (ts.isClassDeclaration(d) && d.name) {
      name = d.name.text + '.constructor';
    } else {
      name = (d.name ? d.name.getText() : '?');
      kind = 'value';
    }
  }
  return { name, file, kind };
}
for (const sf of program.getSourceFiles()) {
  if (!sf.fileName.startsWith(path.join(root, 'src') + path.sep)) continue;
  const visit = node => {
    if (ts.isCallExpression(node) || ts.isNewExpression(node)) {
      const callee = resolve(node.expression);
      if (callee && callee.file.startsWith(root + path.sep) && !callee.file.includes('node_modules')) {
        const line = sf.getLineAndCharacterOfPosition(node.getStart(sf)).line + 1;
        console.log([enclosing(node), callee.name, `${rel(sf.fileName)}:${line}`, callee.kind, rel(callee.file)].join('\t'));
      }
    }
    ts.forEachChild(node, visit);
  };
  visit(sf);
}
