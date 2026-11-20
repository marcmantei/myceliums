"""A scripted stand-in for the Messages API, to check a variant's setup for free.

``agent_eval.py probe`` starts a session exactly as ``run`` would, but with
``ANTHROPIC_BASE_URL`` pointing at this server and a dummy API key, so no
model is called and no account is charged. The server records every request
body (never the headers) and answers with a fixed script:

1. the main agent's first request gets a ``Grep`` call for an identifier the
   index knows, so a PreToolUse hook has something to answer;
2. if the request offers the ``Agent`` tool, the next answer delegates to the
   ``code-explorer`` subagent; the subagent's own first request (recognised
   by its system prompt) gets one ``find_callers`` call, the next a short
   text;
3. everything else ends the turn with a short text.

What the recorded requests show is what ``probe`` reports: which memory files
reached the context (project CLAUDE.md yes, ``~/.claude/CLAUDE.md`` no),
which tools were sent with full schemas, what the hooks added, which agents
the ``Agent`` tool offers and which tools the subagent got.
"""

from __future__ import annotations

import json
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

SUBAGENT_MARKER = "myceliums code graph"


def _sse(events: list[tuple[str, dict]]) -> bytes:
    return "".join(f"event: {name}\ndata: {json.dumps(data)}\n\n" for name, data in events).encode()


def _message(blocks: list[dict], stop: str, model: str) -> bytes:
    ev = [("message_start", {"type": "message_start", "message": {
        "id": "msg_probe", "type": "message", "role": "assistant", "model": model, "content": [],
        "stop_reason": None, "stop_sequence": None,
        "usage": {"input_tokens": 1, "output_tokens": 1}}})]
    for i, b in enumerate(blocks):
        if b["type"] == "text":
            ev.append(("content_block_start", {"type": "content_block_start", "index": i,
                                               "content_block": {"type": "text", "text": ""}}))
            ev.append(("content_block_delta", {"type": "content_block_delta", "index": i,
                                               "delta": {"type": "text_delta", "text": b["text"]}}))
        else:
            ev.append(("content_block_start", {"type": "content_block_start", "index": i, "content_block": {
                "type": "tool_use", "id": b["id"], "name": b["name"], "input": {}}}))
            ev.append(("content_block_delta", {"type": "content_block_delta", "index": i, "delta": {
                "type": "input_json_delta", "partial_json": json.dumps(b["input"])}}))
        ev.append(("content_block_stop", {"type": "content_block_stop", "index": i}))
    ev.append(("message_delta", {"type": "message_delta", "delta": {"stop_reason": stop, "stop_sequence": None},
                                 "usage": {"output_tokens": 1}}))
    ev.append(("message_stop", {"type": "message_stop"}))
    return _sse(ev)


def _system_text(body: dict) -> str:
    sys_ = body.get("system")
    if isinstance(sys_, list):
        return "\n".join(b.get("text", "") for b in sys_ if isinstance(b, dict))
    return sys_ or ""


def _tool_results(body: dict) -> int:
    n = 0
    for m in body.get("messages") or []:
        if m.get("role") == "user" and isinstance(m.get("content"), list):
            n += sum(1 for b in m["content"] if isinstance(b, dict) and b.get("type") == "tool_result")
    return n


class MockAPI:
    def __init__(self, log_dir: Path, grep_pattern: str, callers_symbol: str):
        self.log_dir = log_dir
        self.grep_pattern = grep_pattern
        self.callers_symbol = callers_symbol
        self.n = 0
        self.lock = threading.Lock()
        mock = self

        class Handler(BaseHTTPRequestHandler):
            def log_message(self, *args):  # quiet
                pass

            def do_GET(self):
                self.send_response(404)
                self.end_headers()

            def do_HEAD(self):
                self.send_response(200)
                self.end_headers()

            def do_POST(self):
                length = int(self.headers.get("content-length") or 0)
                raw = self.rfile.read(length)
                try:
                    body = json.loads(raw)
                except ValueError:
                    body = {}
                if not self.path.startswith("/v1/messages") or self.path.startswith("/v1/messages/count_tokens"):
                    out = json.dumps({"input_tokens": 1}).encode()
                    self.send_response(200)
                    self.send_header("content-type", "application/json")
                    self.end_headers()
                    self.wfile.write(out)
                    return
                payload = mock.answer(body)
                self.send_response(200)
                self.send_header("content-type", "text/event-stream")
                self.end_headers()
                self.wfile.write(payload)

        self.server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        self.thread = threading.Thread(target=self.server.serve_forever, daemon=True)

    @property
    def url(self) -> str:
        return f"http://127.0.0.1:{self.server.server_address[1]}"

    def start(self) -> "MockAPI":
        self.thread.start()
        return self

    def stop(self) -> None:
        self.server.shutdown()

    def answer(self, body: dict) -> bytes:
        with self.lock:
            self.n += 1
            n = self.n
        (self.log_dir / f"request{n:02d}.json").write_text(json.dumps(body, indent=1))
        model = body.get("model") or "probe"
        tools = {t.get("name") for t in body.get("tools") or [] if isinstance(t, dict)}
        done = _tool_results(body)
        if not body.get("tools"):
            return _message([{"type": "text", "text": "ok"}], "end_turn", model)
        if SUBAGENT_MARKER in _system_text(body):
            if done == 0 and "mcp__myceliums__find_callers" in tools:
                return _message([{"type": "tool_use", "id": "toolu_probe_sub", "name": "mcp__myceliums__find_callers",
                                  "input": {"symbol_name": self.callers_symbol}}], "tool_use", model)
            return _message([{"type": "text", "text": "subagent done"}], "end_turn", model)
        if done == 0 and "Grep" in tools:
            return _message([{"type": "tool_use", "id": "toolu_probe_grep", "name": "Grep",
                              "input": {"pattern": self.grep_pattern, "output_mode": "files_with_matches"}}],
                            "tool_use", model)
        if done == 1 and "Agent" in tools:
            return _message([{"type": "tool_use", "id": "toolu_probe_agent", "name": "Agent", "input": {
                "subagent_type": "code-explorer", "description": "probe",
                "prompt": f"Who calls {self.callers_symbol}?"}}], "tool_use", model)
        return _message([{"type": "text", "text": 'probe done\n```json\n{"answer": []}\n```'}], "end_turn", model)
