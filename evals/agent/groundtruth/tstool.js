// Ground-truth helper for the TypeScript (hono) tasks, using the TypeScript language service.
// Needs the `typescript` package: set TS_PATH to its directory (or have it resolvable by node).
//   node tstool.js <repoDir> callers <file> <name>   (incoming call hierarchy)
//   node tstool.js <repoDir> callees <file> <name>   (outgoing call hierarchy)
//   node tstool.js <repoDir> refs <file> <name>      (findReferences)
//   node tstool.js <repoDir> unused                   (noUnusedLocals diagnostics, functions only)
// <name> may be "Class.method" or a top-level function/const name.
const ts = require(process.env.TS_PATH || 'typescript');
const path = require('path'), fs = require('fs');
const [repo, cmd, file, name] = process.argv.slice(2);
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
  noUnusedLocals: cmd === 'unused', paths: { 'hono/jsx/jsx-runtime': [path.join(root, 'src/jsx/jsx-runtime.ts')], 'hono/jsx/jsx-dev-runtime': [path.join(root, 'src/jsx/jsx-dev-runtime.ts')], 'hono/*': [path.join(root, 'src/*')] }, baseUrl: root };
const versions = {};
const host = {
  getScriptFileNames: () => files, getScriptVersion: f => '1',
  getScriptSnapshot: f => fs.existsSync(f) ? ts.ScriptSnapshot.fromString(fs.readFileSync(f, 'utf8')) : undefined,
  getCurrentDirectory: () => root, getCompilationSettings: () => options,
  getDefaultLibFileName: o => ts.getDefaultLibFilePath(o), fileExists: ts.sys.fileExists, readFile: ts.sys.readFile,
  readDirectory: ts.sys.readDirectory, directoryExists: ts.sys.directoryExists, getDirectories: ts.sys.getDirectories,
};
const ls = ts.createLanguageService(host, ts.createDocumentRegistry());
const rel = f => path.relative(root, f);
function findDecl(sf, name) {
  const parts = name.split('.');
  let found;
  function visit(node, cls) {
    if (found) return;
    if ((ts.isFunctionDeclaration(node) || ts.isMethodDeclaration(node) || ts.isPropertyDeclaration(node) || ts.isGetAccessor(node)) && node.name) {
      const full = cls ? cls + '.' + node.name.getText(sf) : node.name.getText(sf);
      if (full === name) { found = node.name; return; }
    }
    if (ts.isVariableDeclaration(node) && ts.isIdentifier(node.name) && node.name.text === name) { found = node.name; return; }
    if ((ts.isClassDeclaration(node) || ts.isClassExpression(node)) && node.name) {
      if (node.name.text === name) { found = node.name; return; }
      node.members.forEach(m => visit(m, node.name.text)); return;
    }
    ts.forEachChild(node, n => visit(n, cls));
  }
  visit(sf, null);
  return found;
}
function lineOf(f, pos) { const sf = ls.getProgram().getSourceFile(f); return sf.getLineAndCharacterOfPosition(pos).line + 1; }
if (cmd === 'unused') {
  const prog = ls.getProgram();
  for (const sf of prog.getSourceFiles()) {
    if (!sf.fileName.startsWith(path.join(root, 'src'))) continue;
    for (const d of prog.getSemanticDiagnostics(sf).concat(prog.getSuggestionDiagnostics ? [] : [])) {
      if (d.code === 6133 || d.code === 6196 || d.code === 6198) {
        const msg = ts.flattenDiagnosticMessageText(d.messageText, ' ');
        console.log(`${rel(sf.fileName)}:${sf.getLineAndCharacterOfPosition(d.start).line + 1}\t${msg}`);
      }
    }
  }
  process.exit(0);
}
const abs = path.resolve(root, file);
const sf = ls.getProgram().getSourceFile(abs);
const id = findDecl(sf, name);
if (!id) { console.error('declaration not found'); process.exit(2); }
const pos = id.getStart(sf);
if (cmd === 'refs') {
  const refs = ls.findReferences(abs, pos) || [];
  for (const r of refs) for (const e of r.references) {
    const s = ls.getProgram().getSourceFile(e.fileName);
    const lc = s.getLineAndCharacterOfPosition(e.textSpan.start);
    const lineText = s.text.split('\n')[lc.line].trim().slice(0, 100);
    console.log(`${rel(e.fileName)}:${lc.line + 1}\t${e.isDefinition ? 'DEF' : 'ref'}\t${lineText}`);
  }
} else {
  const items = ls.prepareCallHierarchy(abs, pos);
  const arr = Array.isArray(items) ? items : items ? [items] : [];
  for (const it of arr) {
    console.log('TARGET', it.name, rel(it.file));
    const calls = cmd === 'callers' ? ls.provideCallHierarchyIncomingCalls(abs, it.selectionSpan.start) : ls.provideCallHierarchyOutgoingCalls(abs, it.selectionSpan.start);
    for (const c of calls) {
      const other = cmd === 'callers' ? c.from : c.to;
      const lines = c.fromSpans.map(s => lineOf(cmd === 'callers' ? other.file : abs, s.start));
      console.log(`${other.containerName ? other.containerName + '.' : ''}${other.name}\t${ts.ScriptElementKind[other.kind] || other.kind}\t${rel(other.file)}\tlines=${lines.join(',')}`);
    }
  }
}
