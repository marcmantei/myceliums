// Cross-check a TypeScript gold set against the TypeScript compiler.
//
//   npm install --prefix /tmp/tsc typescript@5
//   NODE_PATH=/tmp/tsc/node_modules node crates/myceliums-benchmarks/graph-gold/verify/ts_calls.cjs
//   NODE_PATH=/tmp/tsc/node_modules node crates/myceliums-benchmarks/graph-gold/verify/ts_calls.cjs typescript-monorepo
//
// The argument names the gold file (default `typescript`). For the monorepo
// fixture (#169) the compiler gets the fixture's own tsconfig.base.json
// (`baseUrl`, `paths`), and each workspace package name is mapped to the
// source file its package.json exports, or to src/index.ts when the
// package.json names a build output (dist/) the fixture does not ship: what
// a build plus node_modules links would give. The external packages the
// fixture imports (lodash, yaml) get an in-memory declaration file.
//
// 1. Symbols: every top-level function, arrow-function constant, class,
//    interface, type alias, method, constructor and interface method must be
//    in the gold set at the same line, and nothing else.
// 2. Calls: every call and `new` expression is resolved with the type checker
//    (getResolvedSignature); a call whose target is a fixture symbol must be a
//    gold edge (primary or also_accept), and every gold edge must be seen.
//    Callers are the enclosing named declaration; arrow functions inside a
//    method belong to the method.
// Exits non-zero on any disagreement.
const ts = require("typescript");
const fs = require("fs");
const path = require("path");

const here = __dirname;
const goldName = process.argv[2] || "typescript";
const gold = JSON.parse(fs.readFileSync(path.join(here, "..", `${goldName}.json`), "utf8"));
const fixture = path.resolve(here, "../../../../tests/fixtures", gold.fixture);
function sources(dir) {
  return fs.readdirSync(dir, { withFileTypes: true }).flatMap((e) => {
    const p = path.join(dir, e.name);
    if (e.isDirectory()) return e.name === "node_modules" ? [] : sources(p);
    return e.name.endsWith(".ts") && !e.name.endsWith(".d.ts") ? [p] : [];
  });
}
const files = sources(fixture);
let options = { strict: true, target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.ESNext, moduleResolution: ts.ModuleResolutionKind.Bundler, lib: ["lib.es2022.d.ts", "lib.dom.d.ts"], noEmit: true };
const externals = path.join(fixture, "__externals__.d.ts");
const externalText = 'declare module "lodash" { export function debounce(fn: () => void, wait: number): () => void; }\n' +
  'declare module "yaml" { export function parse(text: string): unknown; }\n';
if (fs.existsSync(path.join(fixture, "tsconfig.base.json"))) {
  const base = ts.readConfigFile(path.join(fixture, "tsconfig.base.json"), ts.sys.readFile);
  const parsed = ts.parseJsonConfigFileContent(base.config, ts.sys, fixture);
  options = { ...parsed.options, lib: options.lib, noEmit: true };
  const workspaces = JSON.parse(fs.readFileSync(path.join(fixture, "package.json"), "utf8")).workspaces;
  for (const pattern of workspaces) {
    const parent = path.join(fixture, pattern.replace(/\/\*$/, ""));
    for (const dir of fs.readdirSync(parent)) {
      const manifest = JSON.parse(fs.readFileSync(path.join(parent, dir, "package.json"), "utf8"));
      const exportsMap = typeof manifest.exports === "string" ? { ".": manifest.exports } : manifest.exports || {};
      const entries = Object.keys(exportsMap).length ? exportsMap : { ".": "./src/index.ts" };
      for (const [sub, target] of Object.entries(entries)) {
        const name = sub === "." ? manifest.name : `${manifest.name}/${sub.replace(/^\.\//, "")}`;
        options.paths[name] = [path.relative(fixture, path.join(parent, dir, target))];
      }
    }
  }
  files.push(externals);
}
const host = ts.createCompilerHost(options);
const readFile = host.readFile.bind(host);
host.readFile = (f) => (path.resolve(f) === externals ? externalText : readFile(f));
const fileExists = host.fileExists.bind(host);
host.fileExists = (f) => path.resolve(f) === externals || fileExists(f);
const getSourceFile = host.getSourceFile.bind(host);
host.getSourceFile = (f, v) => (path.resolve(f) === externals ? ts.createSourceFile(f, externalText, v) : getSourceFile(f, v));
const program = ts.createProgram(files.map((f) => path.resolve(f)), options, host);
const checker = program.getTypeChecker();
const failures = [];
const diagnostics = ts.getPreEmitDiagnostics(program);
for (const d of diagnostics) failures.push("tsc: " + ts.flattenDiagnosticMessageText(d.messageText, "\n"));

const declIds = new Map(); // declaration node -> id
const found = {};
function rel(sf) { return path.relative(fixture, sf.fileName).split(path.sep).join("/"); }
function lineOf(sf, node) { return sf.getLineAndCharacterOfPosition(node.getStart(sf)).line + 1; }
function record(sf, node, qual, nameNode) {
  const id = `${rel(sf)}::${qual}`;
  declIds.set(node, id);
  found[id] = lineOf(sf, nameNode);
}
for (const sf of program.getSourceFiles()) {
  if (!files.includes(path.resolve(sf.fileName)) || path.resolve(sf.fileName) === externals) continue;
  for (const stmt of sf.statements) {
    if (ts.isFunctionDeclaration(stmt) && stmt.name) record(sf, stmt, stmt.name.text, stmt.name);
    if (ts.isTypeAliasDeclaration(stmt)) record(sf, stmt, stmt.name.text, stmt.name);
    if (ts.isVariableStatement(stmt)) {
      for (const d of stmt.declarationList.declarations) {
        if (d.initializer && ts.isArrowFunction(d.initializer)) { record(sf, d, d.name.text, d.name); declIds.set(d.initializer, `${rel(sf)}::${d.name.text}`); }
      }
    }
    if (ts.isClassDeclaration(stmt) || ts.isInterfaceDeclaration(stmt)) {
      const cls = stmt.name.text;
      record(sf, stmt, cls, stmt.name);
      for (const m of stmt.members) {
        if (ts.isConstructorDeclaration(m)) record(sf, m, `${cls}.constructor`, m.getFirstToken(sf));
        if ((ts.isMethodDeclaration(m) || ts.isMethodSignature(m)) && m.name) record(sf, m, `${cls}.${m.name.text}`, m.name);
      }
    }
  }
}

const goldSymbols = Object.fromEntries(gold.symbols.map((s) => [s.id, s.line]));
for (const [id, line] of Object.entries(found)) {
  if (!(id in goldSymbols)) failures.push(`symbol missing from gold: ${id} (line ${line})`);
  else if (goldSymbols[id] !== line) failures.push(`symbol line differs: ${id} gold ${goldSymbols[id]} tsc ${line}`);
}
for (const id of Object.keys(goldSymbols)) if (!(id in found)) failures.push(`gold symbol not in source: ${id}`);

function caller(node) {
  for (let n = node.parent; n; n = n.parent) {
    if (declIds.has(n) && !ts.isClassDeclaration(n) && !ts.isInterfaceDeclaration(n)) return declIds.get(n);
  }
  return null;
}
function target(node) {
  const sig = checker.getResolvedSignature(node);
  let decl = sig && sig.declaration;
  if (!decl && ts.isNewExpression(node)) {
    // A class without a declared constructor: the target is the class.
    let sym = checker.getSymbolAtLocation(node.expression);
    if (sym && sym.flags & ts.SymbolFlags.Alias) sym = checker.getAliasedSymbol(sym);
    decl = sym && sym.valueDeclaration;
  }
  return decl && declIds.get(decl);
}
const observed = new Set();
function visit(sf, node) {
  if (ts.isCallExpression(node) || ts.isNewExpression(node)) {
    const from = caller(node);
    const to = target(node);
    if (from && to) observed.add(`${from} -> ${to}`);
  }
  ts.forEachChild(node, (child) => visit(sf, child));
}
for (const sf of program.getSourceFiles()) if (files.includes(path.resolve(sf.fileName)) && path.resolve(sf.fileName) !== externals) visit(sf, sf);

const accepted = new Map();
gold.calls.forEach((c, i) => [c.to, ...(c.also_accept || [])].forEach((t) => accepted.set(`${c.from} -> ${t}`, i)));
const dynamic = new Set((gold.dynamic || []).map((d) => `${d.from} -> ${d.to}`));
const seen = new Set();
for (const pair of [...observed].sort()) {
  if (dynamic.has(pair)) continue;
  if (accepted.has(pair)) seen.add(accepted.get(pair));
  else failures.push(`resolved call not in gold: ${pair}`);
}
gold.calls.forEach((c, i) => { if (!seen.has(i)) failures.push(`gold edge never resolved: ${c.from} -> ${c.to}`); });
failures.forEach((f) => console.log(f));
console.log(`${Object.keys(found).length} symbols, ${gold.calls.length} gold edges, ${failures.length} disagreements`);
process.exit(failures.length ? 1 : 0);
