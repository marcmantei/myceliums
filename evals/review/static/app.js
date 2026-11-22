"use strict";
// Label review UI (#164). Every value from the data is put into the page with
// textContent / createElement only: no innerHTML, no external resources.

const TOKEN = document.querySelector('meta[name="review-token"]').content;
const S = { queue: [], pos: 0, current: null, summary: null, warnings: [], stale: 0,
            view: location.pathname === "/summary" ? "summary" : "review",
            commentOpen: false, advanceAfterComment: false, busy: false };

const FIX_HINTS = {
  "retrieval-label": "N alone removes this label. To swap it for another symbol:\n  replace: path/to/file.py::Class.method — why",
  "retrieval-complete": "Name each missing answer on its own line:\n  add: path/to/file.py::Class.method — why",
  "graph-symbol": "N removes the symbol (apply.py lists it for a hand fix if an edge or trap still names it).",
  "graph-call": "N removes the edge. If the call binds to another fixture symbol:\n  replace: path/file::Symbol",
  "graph-dynamic": "Fixed by hand: say what the call really is.",
  "graph-trap": "Fixed by hand: say which edge this call should produce.",
  "held-out-pair": "Give the right targets:\n  targets: path/file.py::name:LINE, other/file.go::Name:LINE\n  targets: none",
  "task-gold": "N removes the answer from the gold set. To make it optional instead:\n  optional — why",
  "task-optional": "N removes it from the optional list.",
  "task-complete": "Fixed by hand (a gold answer needs evidence lines): name what is missing.",
};

function el(tag, cls, text) {
  const e = document.createElement(tag);
  if (cls) e.className = cls;
  if (text !== undefined && text !== null) e.textContent = String(text);
  return e;
}
function clear(node) { while (node.firstChild) node.removeChild(node.firstChild); }

async function api(path, body) {
  const opts = body === undefined ? {} : {
    method: "POST",
    headers: { "Content-Type": "application/json", "X-Review-Token": TOKEN },
    body: JSON.stringify(body),
  };
  const res = await fetch(path, opts);
  const data = await res.json();
  if (!res.ok) throw new Error(data.error || res.statusText);
  return data;
}

async function loadState() {
  const st = await api("/api/state");
  S.queue = st.queue; S.summary = st.summary; S.warnings = st.warnings; S.stale = st.stale_decisions;
  return st;
}

function decidedCount() { return S.queue.filter(q => q.decision).length; }

function renderTop() {
  const n = S.queue.length, d = decidedCount();
  document.getElementById("bar").style.width = n ? (100 * d / n).toFixed(1) + "%" : "0";
  const st = document.getElementById("status");
  clear(st);
  st.appendChild(el("span", null, `${d} of ${n} decided`));
  if (S.view === "review" && n) st.appendChild(el("span", null, `item ${S.pos + 1} of ${n}`));
  const link = el("a", null, S.view === "review" ? "summary (S)" : "back to review (S)");
  link.href = S.view === "review" ? "/summary" : "/";
  link.addEventListener("click", ev => { ev.preventDefault(); toggleView(); });
  st.appendChild(link);
  if (S.stale) st.appendChild(el("span", "v-doubtful", `${S.stale} decisions on changed claims are open again`));
}

function langOf(file) {
  const m = /\.([a-z]+)$/.exec(file || "");
  return m ? m[1] : "";
}

const KW = new Set(("def class return if else elif for while func function const let var import from export type " +
  "interface struct fn pub impl async await new self this package public private protected static void try except " +
  "catch finally raise throw with yield lambda in not and or is None null nil true false True False go defer range " +
  "match use mod enum extends implements switch case break continue default select chan map mut ref where trait final " +
  "abstract override readonly undefined super pass global nonlocal assert del").split(" "));

function highlight(text, lang) {
  const frag = document.createDocumentFragment();
  const hash = lang === "py";
  const rx = hash
    ? /(#.*$)|("""|'''|"(?:\\.|[^"\\])*"|'(?:\\.|[^'\\])*')|\b(\d+(?:\.\d+)?)\b|\b([A-Za-z_]\w*)\b/g
    : /(\/\/.*$|\/\*.*?\*\/)|("(?:\\.|[^"\\])*"|'(?:\\.|[^'\\])*'|`(?:\\.|[^`\\])*`)|\b(\d+(?:\.\d+)?)\b|\b([A-Za-z_]\w*)\b/g;
  let last = 0, m;
  while ((m = rx.exec(text)) !== null) {
    let cls = null;
    if (m[1]) cls = "t-com"; else if (m[2]) cls = "t-str"; else if (m[3]) cls = "t-num";
    else if (m[4] && KW.has(m[4])) cls = "t-kw";
    if (!cls) continue;
    if (m.index > last) frag.appendChild(document.createTextNode(text.slice(last, m.index)));
    frag.appendChild(el("span", cls, m[0]));
    last = m.index + m[0].length;
  }
  if (last < text.length) frag.appendChild(document.createTextNode(text.slice(last)));
  return frag;
}

function codeBlock(ev) {
  const box = el("div", "card");
  box.appendChild(el("div", "ev-head", `${ev.source || ""}/${ev.file}${ev.line ? ":" + ev.line : ""}`));
  if (ev.note) box.appendChild(el("div", "ev-note", ev.note));
  if (ev.missing) { box.appendChild(el("div", "v-wrong", ev.missing)); return box; }
  const pre = el("pre", "code");
  const focus = new Set(ev.focus || []);
  const lang = langOf(ev.file);
  (ev.lines || []).forEach((text, i) => {
    const no = ev.start + i;
    const line = el("span", focus.has(no) ? "ln focus" : "ln");
    line.appendChild(el("span", "no", no));
    line.appendChild(highlight(text, lang));
    pre.appendChild(line);
  });
  box.appendChild(pre);
  return box;
}

function ctxValue(v) {
  if (Array.isArray(v)) return v.length ? v.join("\n") : "—";
  if (v && typeof v === "object") return JSON.stringify(v);
  return v === "" || v === null || v === undefined ? "—" : String(v);
}

function renderReview() {
  const main = document.getElementById("main");
  clear(main);
  if (!S.queue.length) { main.appendChild(el("p", null, "The queue is empty.")); return; }
  const data = S.current;
  if (!data) return;
  const it = data.item;
  const head = el("div", "card");
  const meta = el("div", "meta");
  meta.appendChild(el("span", null, it.dataset));
  meta.appendChild(el("span", null, it.kind));
  meta.appendChild(el("span", "badge v-" + data.verdict,
    data.prescreen ? `pre-screen: ${data.verdict}` : "no pre-screen (counts as doubtful)"));
  if (data.sampled) meta.appendChild(el("span", "badge v-none", "sampled from confirmed"));
  meta.appendChild(el("span", data.decision ? "badge d-" + data.decision : "badge v-none",
    data.decision ? `your decision: ${data.decision}` : "not decided"));
  head.appendChild(meta);
  head.appendChild(el("div", "claim", it.claim));
  const lab = el("div", "label");
  lab.appendChild(el("strong", null, "Label: "));
  lab.appendChild(document.createTextNode(it.label));
  if (it.location) lab.appendChild(document.createTextNode(`   ·   ${it.location}`));
  head.appendChild(lab);
  const src = el("div", "meta", `source: ${it.source.file}${it.source.line ? ":" + it.source.line : ""}  ${it.source.pointer}`);
  head.appendChild(src);
  const dl = el("dl", "ctx");
  for (const [k, v] of Object.entries(it.context || {})) {
    dl.appendChild(el("dt", null, k));
    dl.appendChild(el("dd", null, ctxValue(v)));
  }
  head.appendChild(dl);
  if (data.comment) head.appendChild(el("div", "comment-show", "Your comment: " + data.comment));
  main.appendChild(head);

  const pre = el("div", "card");
  if (data.prescreen) {
    const p = data.prescreen;
    pre.appendChild(el("div", "pre-title v-" + p.verdict, `Pre-screen: ${p.verdict}`));
    pre.appendChild(el("div", "reason", p.reason || ""));
    if (Array.isArray(p.evidence) && p.evidence.length)
      pre.appendChild(el("div", "meta", "evidence: " + p.evidence.join(", ")));
    if (p.suggested_fix) pre.appendChild(el("div", "fix", "suggested fix: " + p.suggested_fix));
  } else {
    pre.appendChild(el("div", "pre-title v-none", "No pre-screen for this item"));
  }
  main.appendChild(pre);
  (it.evidence || []).forEach(ev => main.appendChild(codeBlock(ev)));
}

function renderSummary() {
  const main = document.getElementById("main");
  clear(main);
  const s = S.summary;
  const card = el("div", "card");
  card.appendChild(el("div", "claim", "Summary"));
  card.appendChild(el("p", null, `Acceptance rule: ${s.rule}. Unsure counts as an error in the sample. ` +
    `When a dataset's sample is fully decided and the rule fails, the sample is widened by another ` +
    `${Math.round(s.rate * 100)} % of its confirmed items.`));
  if (S.warnings.length) card.appendChild(el("p", "v-doubtful", `${S.warnings.length} pre-screen warnings (see the server's output)`));
  main.appendChild(card);
  const table = el("table", "sum");
  const cols = ["dataset", "items", "pre-screen c/d/w", "reviewed / queue", "wrong among flagged",
                "sample reviewed / size", "sample errors", "error rate", "Wilson 95 %", "status"];
  const tr = el("tr");
  cols.forEach(c => tr.appendChild(el("th", null, c)));
  table.appendChild(tr);
  const pct = x => (100 * x).toFixed(1) + " %";
  const STATUS = { "no-confirmed": "nothing to accept by sample", "sample-open": "sample not fully reviewed",
                   "accepted": "unsampled confirmed items accepted", "widen": "rule failed: sample widened",
                   "all-confirmed-reviewed": "every confirmed item reviewed" };
  for (const d of s.datasets) {
    const row = el("tr");
    const v = d.verdicts;
    const cells = [d.dataset, d.items,
      `${v.confirmed}/${v.doubtful}/${v.wrong}` + (d.prescreened < d.items ? ` (${d.items - d.prescreened} unscreened)` : ""),
      `${d.reviewed} / ${d.queue}`, `${d.flagged_wrong} of ${d.flagged_reviewed}`,
      `${d.sample_reviewed} / ${d.sample_size}`, d.sample_errors,
      d.sample_error_rate === null ? "—" : pct(d.sample_error_rate),
      d.wilson95 ? `${pct(d.wilson95[0])} – ${pct(d.wilson95[1])}` : "—"];
    cells.forEach(c => row.appendChild(el("td", null, c)));
    row.appendChild(el("td", "st st-" + d.status, STATUS[d.status] || d.status));
    table.appendChild(row);
  }
  main.appendChild(table);
}

function render() {
  renderTop();
  if (S.view === "summary") renderSummary(); else renderReview();
}

async function show(pos) {
  if (!S.queue.length) { S.current = null; render(); return; }
  pos = Math.max(0, Math.min(S.queue.length - 1, pos));
  S.pos = pos;
  history.replaceState(null, "", (S.view === "summary" ? "/summary" : "/") + "#" + (pos + 1));
  S.current = await api("/api/item?id=" + encodeURIComponent(S.queue[pos].id));
  render();
  window.scrollTo(0, 0);
}

function nextOpen(from) {
  for (let i = from + 1; i < S.queue.length; i++) if (!S.queue[i].decision) return i;
  return Math.min(from + 1, S.queue.length - 1);
}

async function afterSave(res) {
  if (res.queue_changed) {
    const id = S.queue[S.pos].id;
    await loadState();
    const at = S.queue.findIndex(q => q.id === id);
    if (at >= 0) S.pos = at;
  } else {
    S.queue[S.pos].decision = res.saved.decision;
    const st = await api("/api/state");
    S.summary = st.summary;
  }
}

async function decide(decision) {
  if (S.busy || !S.current) return;
  S.busy = true;
  try {
    const res = await api("/api/decide", { id: S.current.item.id, decision });
    await afterSave(res);
    if (decision === "wrong") {
      S.current.decision = "wrong";
      render();
      openComment(true);
    } else {
      await show(nextOpen(S.pos));
    }
  } catch (e) { alert("Not saved: " + e.message); }
  finally { S.busy = false; }
}

function openComment(advanceAfter) {
  if (!S.current) return;
  S.commentOpen = true;
  S.advanceAfterComment = advanceAfter;
  const box = document.getElementById("comment-box");
  const ta = document.getElementById("comment");
  const p = S.current.prescreen;
  const prefill = !S.current.comment && advanceAfter && p && p.suggested_fix;
  ta.value = S.current.comment || (prefill ? p.suggested_fix : "");
  document.getElementById("comment-hint").textContent = (FIX_HINTS[S.current.item.kind] || "") +
    (prefill ? "\nPre-filled with the pre-screen's suggested fix: Enter keeps it, typing replaces it, Esc saves none." : "");
  box.hidden = false;
  ta.focus();
  ta.select();
}

async function closeComment(save) {
  const box = document.getElementById("comment-box");
  const ta = document.getElementById("comment");
  const advance = S.advanceAfterComment;
  S.commentOpen = false;
  box.hidden = true;
  try {
    if (save) {
      const res = await api("/api/decide", { id: S.current.item.id, comment: ta.value.trim() });
      await afterSave(res);
    }
    if (advance) await show(nextOpen(S.pos)); else await show(S.pos);
  } catch (e) { alert("Comment not saved: " + e.message); }
}

async function toggleView() {
  S.view = S.view === "review" ? "summary" : "review";
  if (S.view === "summary") { await loadState(); history.replaceState(null, "", "/summary#" + (S.pos + 1)); render(); }
  else await show(S.pos);
}

document.addEventListener("keydown", ev => {
  if (S.commentOpen) {
    if (ev.key === "Enter" && !ev.shiftKey) { ev.preventDefault(); closeComment(true); }
    else if (ev.key === "Escape") { ev.preventDefault(); closeComment(false); }
    return;
  }
  if (ev.metaKey || ev.ctrlKey || ev.altKey) return;
  const k = ev.key;
  if (k === "s" || k === "S") { ev.preventDefault(); toggleView(); return; }
  if (S.view !== "review") return;
  if (k === "j" || k === "J") { ev.preventDefault(); decide("correct"); }
  else if (k === "n" || k === "N") { ev.preventDefault(); decide("wrong"); }
  else if (k === "u" || k === "U") { ev.preventDefault(); decide("unsure"); }
  else if (k === "k" || k === "K") { ev.preventDefault(); openComment(false); }
  else if (k === "ArrowRight") { ev.preventDefault(); show(S.pos + 1); }
  else if (k === "ArrowLeft") { ev.preventDefault(); show(S.pos - 1); }
});

(async function init() {
  const st = await loadState();
  const fromHash = parseInt((location.hash || "").slice(1), 10);
  const pos = Number.isFinite(fromHash) && fromHash >= 1 ? fromHash - 1 : st.start;
  if (S.view === "summary") { S.pos = pos; render(); } else await show(pos);
})();
