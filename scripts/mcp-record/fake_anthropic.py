#!/usr/bin/env python3
"""A scripted stand-in for the Anthropic Messages API, for recording MCP fixtures.

The real Claude Code CLI is pointed at this server with ANTHROPIC_BASE_URL, so
recording the CLI's MCP conversation needs no model credential and no network,
and the tool calls the CLI makes are the same on every run
(`images/claude/VERIFY.md`, "Recording the MCP conformance fixtures").

Each main-loop request (one that offers the mars-orchestrator tools) is answered
by the next step of SCRIPT, chosen by how many tool results the conversation
already carries; any other request (a title, a quota probe) gets a one-word
text answer. Nothing here is a credential: the CLI is given an obviously fake
key and this server never looks at it.
"""

import json
import sys
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

PREFIX = "mcp__mars-orchestrator__"

# One tool call per step, then a closing text turn. The unknown tool is there
# to observe whether the CLI forwards a name it was never offered: it answers
# that itself, so the recording shows no request for it.
SCRIPT = [
    (PREFIX + "ready", {}),
    (PREFIX + "get_task", {"task": 999999}),
    (PREFIX + "no_such_tool", {}),
]


def tool_results(body):
    count = 0
    for message in body.get("messages", []):
        content = message.get("content")
        if isinstance(content, list):
            count += sum(1 for block in content if block.get("type") == "tool_result")
    return count


def offers_mars_tools(body):
    return any(tool.get("name", "").startswith(PREFIX) for tool in body.get("tools", []))


def reply_blocks(body):
    """The content of the answer, and its stop reason."""
    if offers_mars_tools(body):
        step = tool_results(body)
        if step < len(SCRIPT):
            name, arguments = SCRIPT[step]
            block = {"type": "tool_use", "id": f"toolu_fake{step + 1:04d}", "name": name, "input": arguments}
            return [block], "tool_use"
    return [{"type": "text", "text": "done"}], "end_turn"


def usage():
    return {"input_tokens": 1, "output_tokens": 1, "cache_creation_input_tokens": 0, "cache_read_input_tokens": 0}


class Handler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, fmt, *args):
        sys.stderr.write("fake-anthropic: " + (fmt % args) + "\n")

    def read_body(self):
        length = int(self.headers.get("content-length") or 0)
        raw = self.rfile.read(length) if length else b"{}"
        try:
            return json.loads(raw)
        except ValueError:
            return {}

    def send_json(self, status, value):
        data = json.dumps(value).encode()
        self.send_response(status)
        self.send_header("content-type", "application/json")
        self.send_header("content-length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def do_GET(self):
        self.send_json(404, {"type": "error", "error": {"type": "not_found_error", "message": "fake"}})

    def do_POST(self):
        body = self.read_body()
        path = self.path.split("?", 1)[0]
        if path.endswith("/count_tokens"):
            return self.send_json(200, {"input_tokens": 1})
        if not path.endswith("/v1/messages"):
            return self.do_GET()

        blocks, stop = reply_blocks(body)
        model = body.get("model", "claude-fake")
        message = {"id": "msg_fake", "type": "message", "role": "assistant", "model": model,
                   "content": blocks, "stop_reason": stop, "stop_sequence": None, "usage": usage()}
        if not body.get("stream"):
            return self.send_json(200, message)

        self.send_response(200)
        self.send_header("content-type", "text/event-stream")
        self.send_header("cache-control", "no-cache")
        self.send_header("connection", "close")
        self.end_headers()
        self.close_connection = True

        def event(kind, data):
            self.wfile.write(f"event: {kind}\ndata: {json.dumps(data)}\n\n".encode())

        event("message_start", {"type": "message_start",
                                "message": {**message, "content": [], "stop_reason": None}})
        for index, block in enumerate(blocks):
            if block["type"] == "tool_use":
                event("content_block_start", {"type": "content_block_start", "index": index,
                                              "content_block": {**block, "input": {}}})
                event("content_block_delta", {"type": "content_block_delta", "index": index,
                                              "delta": {"type": "input_json_delta",
                                                        "partial_json": json.dumps(block["input"])}})
            else:
                event("content_block_start", {"type": "content_block_start", "index": index,
                                              "content_block": {"type": "text", "text": ""}})
                event("content_block_delta", {"type": "content_block_delta", "index": index,
                                              "delta": {"type": "text_delta", "text": block["text"]}})
            event("content_block_stop", {"type": "content_block_stop", "index": index})
        event("message_delta", {"type": "message_delta", "delta": {"stop_reason": stop, "stop_sequence": None},
                                "usage": {"output_tokens": 1}})
        event("message_stop", {"type": "message_stop"})
        self.wfile.flush()


if __name__ == "__main__":
    port = int(sys.argv[1]) if len(sys.argv) > 1 else 7099
    ThreadingHTTPServer(("0.0.0.0", port), Handler).serve_forever()
