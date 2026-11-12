// Ground-truth helper for hono-dead-utils: for each exported top-level function/const in files under
// <dir>, report the ones with no reference from non-test files (TypeScript findReferences).
//   TS_PATH=/path/to/node_modules/typescript node ts_exports_unused.js <repoDir> src/utils
const ts = require(process.env.TS_PATH || 'typescript');
const path = require('path'), fs = require('fs');
const [repo, dir] = process.argv.slice(2);
const root = path.resolve(repo);
function walk(d, out) { for (const e of fs.readdirSync(d, { withFileTypes: true })) { if (e.name === 'node_modules' || e.name.startsWith('.')) continue; const p = path.join(d, e.name); if (e.isDirectory()) walk(p, out); else if (/\.(ts|tsx)$/.test(e.name) && !e.name.endsWith('.d.ts')) out.push(p); } return out; }
const files = walk(path.join(root, 'src'), []);
const options = { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.ESNext, moduleResolution: ts.ModuleResolutionKind.Bundler, strict: true, skipLibCheck: true, jsx: ts.JsxEmit.ReactJSX, jsxImportSource: 'hono/jsx', noEmit: true, baseUrl: root, paths: { 'hono/*': [path.join(root, 'src/*')] } };
const host = { getScriptFileNames: () => files, getScriptVersion: () => '1', getScriptSnapshot: f => fs.existsSync(f) ? ts.ScriptSnapshot.fromString(fs.readFileSync(f, 'utf8')) : undefined, getCurrentDirectory: () => root, getCompilationSettings: () => options, getDefaultLibFileName: o => ts.getDefaultLibFilePath(o), fileExists: ts.sys.fileExists, readFile: ts.sys.readFile, readDirectory: ts.sys.readDirectory, directoryExists: ts.sys.directoryExists, getDirectories: ts.sys.getDirectories };
const ls = ts.createLanguageService(host, ts.createDocumentRegistry());
const prog = ls.getProgram();
const isTest = f => /\.test\.tsx?$/.test(f);
for (const f of files.filter(f => f.startsWith(path.resolve(root, dir) + path.sep) && !isTest(f))) {
  const sf = prog.getSourceFile(f);
  for (const st of sf.statements) {
    const exported = st.modifiers && st.modifiers.some(m => m.kind === ts.SyntaxKind.ExportKeyword);
    if (!exported) continue;
    let ids = [];
    if (ts.isFunctionDeclaration(st) && st.name) ids.push(st.name);
    if (ts.isVariableStatement(st)) for (const d of st.declarationList.declarations) if (ts.isIdentifier(d.name)) ids.push(d.name);
    if (ts.isClassDeclaration(st) && st.name) ids.push(st.name);
    for (const id of ids) {
      const refs = ls.findReferences(f, id.getStart(sf)) || [];
      let inFile = 0, prodOther = 0, testRefs = 0, importOnly = 0;
      const where = new Set();
      for (const r of refs) for (const e of r.references) {
        if (e.isDefinition) continue;
        if (isTest(e.fileName)) { testRefs++; continue; }
        const s = prog.getSourceFile(e.fileName);
        const node = ts.getTokenAtPosition ? null : null;
        const lineText = s.text.slice(s.getLineStarts()[s.getLineAndCharacterOfPosition(e.textSpan.start).line]).split('\n')[0];
        if (/^\s*(import|export)\b/.test(lineText) || /^\s*[A-Za-z_$][\w$]*,?\s*$/.test(lineText)) { importOnly++; continue; }
        if (e.fileName === f) inFile++; else { prodOther++; where.add(path.relative(root, e.fileName)); }
      }
      if (prodOther === 0 && inFile === 0) console.log(`${path.relative(root, f)}\t${id.text}\tin_file=${inFile}\tother_prod=${prodOther}\timport_or_reexport=${importOnly}\ttests=${testRefs}`);
    }
  }
}
