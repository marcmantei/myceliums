#!/usr/bin/env python3
"""Local review UI for the agent-drafted labels (#164).

    python3 evals/review/serve.py            # prints http://127.0.0.1:<port>/

Binds 127.0.0.1 only, on a random free port (``--port`` to fix one). No
external assets, no telemetry, nothing leaves the machine. One item per
screen; keys J correct, N wrong, U unsure, K comment, ←/→ navigate, S summary.
Every decision is appended to ``decisions.jsonl`` at once, so the review can
be stopped at any time and resumed by starting the server again.

Queue: every item the pre-screen marked doubtful or wrong (an item without a
pre-screen counts as doubtful) plus a seeded random sample of the confirmed
ones per dataset (``sample.json`` records seed, rate and the drawn ids). When
a dataset's sample is fully decided and its Wilson 95 % upper error bound is
not below the threshold, the sample is widened automatically.
Python 3.10+ standard library only.
"""

from __future__ import annotations

import argparse
import json
import secrets
import sys
from http import HTTPStatus
from http.server import BaseHTTPRequestHandler, HTTPServer
from pathlib import Path
from urllib.parse import parse_qs, urlparse

sys.path.insert(0, str(Path(__file__).resolve().parent))
import common  # noqa: E402

STATIC = Path(__file__).resolve().parent / "static"
CSP = ("default-src 'none'; script-src 'self'; style-src 'self'; connect-src 'self'; "
       "img-src 'none'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'")
MAX_BODY = 64 * 1024


class App:
    """Server state: the review, its files and the session token."""

    def __init__(self, items_file: Path, prescreen_dir: Path, decisions_file: Path, sample_file: Path,
                 seed: int, rate: float, max_upper: float, reviewer: str):
        items = common.load_items(items_file)
        by_id = {it["id"]: it for it in items}
        prescreen, self.warnings = common.load_prescreen(prescreen_dir, by_id)
        self.sample_file = sample_file
        sample = common.Sample.load_or_create(sample_file, items, prescreen, seed, rate)
        decisions = common.DecisionStore(decisions_file)
        self.review = common.Review(items, prescreen, decisions, sample, max_upper)
        self.reviewer = reviewer
        self.token = secrets.token_urlsafe(24)
        self.stale = decisions.stale(by_id)
        for ds in list(self.review.confirmed):
            if self.review.widen_if_needed(ds):
                sample.save(sample_file)

    def state(self) -> dict:
        r = self.review
        return {
            "queue": [{"id": rid, "dataset": r.by_id[rid]["dataset"],
                       "verdict": common.verdict_of(rid, r.prescreen),
                       "sampled": rid in r.sampled, "decision": r.decision(rid)} for rid in r.queue],
            "start": r.first_open(),
            "summary": r.summary(),
            "warnings": self.warnings[:50],
            "stale_decisions": len(self.stale),
        }

    def item(self, item_id: str) -> dict | None:
        r = self.review
        it = r.by_id.get(item_id)
        if it is None:
            return None
        rec = r.decisions.get(item_id)
        current = r.decisions.effective(it)
        return {
            "item": it,
            "prescreen": r.prescreen.get(item_id),
            "verdict": common.verdict_of(item_id, r.prescreen),
            "sampled": item_id in r.sampled,
            "decision": current["decision"] if current else None,
            "comment": (rec or {}).get("comment", "") if rec and rec.get("claim_digest") == common.claim_digest(it) else "",
        }

    def decide(self, body: dict) -> tuple[int, dict]:
        r = self.review
        it = r.by_id.get(body.get("id"))
        if it is None:
            return 404, {"error": "unknown item"}
        decision = body.get("decision")
        comment = body.get("comment")
        if decision is not None and decision not in common.DECISIONS:
            return 400, {"error": "bad decision"}
        if comment is not None and (not isinstance(comment, str) or len(comment) > 8000):
            return 400, {"error": "bad comment"}
        if decision is None and comment is None:
            return 400, {"error": "nothing to save"}
        rec = r.decisions.record(it, decision, comment, self.reviewer)
        widened = r.widen_if_needed(it["dataset"])
        if widened:
            r.sample.save(self.sample_file)
        return 200, {"saved": rec, "widened": widened, "queue_changed": bool(widened)}


def make_handler(app: App, port_ref: list[int]):
    class Handler(BaseHTTPRequestHandler):
        server_version = "label-review"
        sys_version = ""

        def log_message(self, fmt, *args):  # quiet: nothing to log, nothing sent anywhere
            pass

        def _host_ok(self) -> bool:
            host = self.headers.get("Host", "")
            return host in (f"127.0.0.1:{port_ref[0]}", f"localhost:{port_ref[0]}")

        def _send(self, status: int, body: bytes, ctype: str) -> None:
            self.send_response(status)
            self.send_header("Content-Type", ctype)
            self.send_header("Content-Length", str(len(body)))
            self.send_header("Content-Security-Policy", CSP)
            self.send_header("X-Content-Type-Options", "nosniff")
            self.send_header("Referrer-Policy", "no-referrer")
            self.send_header("Cache-Control", "no-store")
            self.send_header("Cross-Origin-Resource-Policy", "same-origin")
            self.end_headers()
            self.wfile.write(body)

        def _json(self, status: int, value) -> None:
            self._send(status, json.dumps(value, ensure_ascii=False).encode("utf-8"),
                       "application/json; charset=utf-8")

        def do_GET(self):
            if not self._host_ok():
                return self._send(403, b"forbidden", "text/plain")
            url = urlparse(self.path)
            if url.path in ("/", "/summary"):
                html = (STATIC / "index.html").read_text(encoding="utf-8").replace("__TOKEN__", app.token)
                return self._send(200, html.encode("utf-8"), "text/html; charset=utf-8")
            if url.path == "/app.js":
                return self._send(200, (STATIC / "app.js").read_bytes(), "text/javascript; charset=utf-8")
            if url.path == "/app.css":
                return self._send(200, (STATIC / "app.css").read_bytes(), "text/css; charset=utf-8")
            if url.path == "/api/state":
                return self._json(200, app.state())
            if url.path == "/api/item":
                rid = parse_qs(url.query).get("id", [""])[0]
                data = app.item(rid)
                return self._json(200, data) if data else self._json(404, {"error": "unknown item"})
            return self._send(404, b"not found", "text/plain")

        def do_POST(self):
            if not self._host_ok():
                return self._send(403, b"forbidden", "text/plain")
            if self.headers.get("X-Review-Token") != app.token:
                return self._json(403, {"error": "bad token"})
            if not self.headers.get("Content-Type", "").startswith("application/json"):
                return self._json(415, {"error": "json only"})
            length = int(self.headers.get("Content-Length") or 0)
            if length <= 0 or length > MAX_BODY:
                return self._json(413, {"error": "bad length"})
            try:
                body = json.loads(self.rfile.read(length))
            except json.JSONDecodeError:
                return self._json(400, {"error": "bad json"})
            if urlparse(self.path).path == "/api/decide" and isinstance(body, dict):
                status, out = app.decide(body)
                return self._json(status, out)
            return self._json(HTTPStatus.NOT_FOUND, {"error": "not found"})

    return Handler


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(description="Local label review UI (#164)")
    ap.add_argument("--port", type=int, default=0, help="default: a random free port")
    ap.add_argument("--items", type=Path, default=common.ITEMS_FILE)
    ap.add_argument("--prescreen", type=Path, default=common.PRESCREEN_DIR)
    ap.add_argument("--decisions", type=Path, default=common.DECISIONS_FILE)
    ap.add_argument("--sample", type=Path, default=common.SAMPLE_FILE)
    ap.add_argument("--seed", type=int, default=common.DEFAULT_SEED,
                    help="sample seed; used only when sample.json does not exist yet")
    ap.add_argument("--rate", type=float, default=common.SAMPLE_RATE)
    ap.add_argument("--max-upper", type=float, default=common.ACCEPT_MAX_UPPER)
    ap.add_argument("--reviewer", default="marc")
    args = ap.parse_args(argv)

    app = App(args.items, args.prescreen, args.decisions, args.sample, args.seed, args.rate,
              args.max_upper, args.reviewer)
    port_ref = [0]
    server = HTTPServer(("127.0.0.1", args.port), make_handler(app, port_ref))
    port_ref[0] = server.server_address[1]
    s = app.review.summary()
    for w in app.warnings[:10]:
        print(f"warning: {w}", file=sys.stderr)
    if app.stale:
        print(f"note: {len(app.stale)} recorded decisions refer to claims that have changed; "
              f"those items are open again", file=sys.stderr)
    print(f"queue: {s['queue']} items, {s['reviewed']} decided (sample seed {s['seed']}, rate {s['rate']})")
    print(f"review at http://127.0.0.1:{port_ref[0]}/   (Ctrl-C to stop; progress is saved after every key)",
          flush=True)
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        pass
    finally:
        server.server_close()
    return 0


if __name__ == "__main__":
    sys.exit(main())
