// Calling an MCP tool as a running session, from the host.
//
// The stub image calls no MCP tool at all (`images/stub/claude`), so a
// scenario that needs what only an agent does — a task filed by a session,
// with `created_by_session_id` set — stands in for the agent the way
// `commitInSessionWorkClone` does for its commits: it reads the bearer token
// the orchestrator wrote into `<DATA_DIR>/sessions/<sid>/mcp.json` for that
// session's CLI (`ARCHITECTURE.md`, "Storage") and makes the one POST the CLI
// makes for a tool call (`SPEC.md`, "MCP tool contracts"). The config's URL
// names the host gateway the container reaches the listener through; from
// the host the same port answers on loopback, which the listener admits as
// well (`README.md`, "Configuration", `MCP_URL`).

import { readFileSync } from "node:fs";
import { join } from "node:path";

import type { Task } from "../../src/types";
import { dataDir } from "./env";

/** The protocol revision the pinned CLI speaks (`orchestrator/tests/fixtures/mcp/`). */
const PROTOCOL_VERSION = "2026-07-28";

interface McpConfig {
  mcpServers: Record<
    string,
    { url: string; headers: { Authorization: string } } | undefined
  >;
}

/** The session's MCP endpoint, as reachable from the host, and its bearer. */
function sessionEndpoint(sessionId: string): {
  url: string;
  authorization: string;
} {
  const path = join(dataDir(), "sessions", sessionId, "mcp.json");
  const config = JSON.parse(readFileSync(path, "utf8")) as McpConfig;
  const server = config.mcpServers["mars-orchestrator"];
  if (server === undefined) {
    throw new Error(`${path} names no mars-orchestrator server`);
  }
  const url = new URL(server.url);
  url.hostname = "localhost";
  return { url: url.toString(), authorization: server.headers.Authorization };
}

/** The JSON-RPC answer in either body the transport may send. */
function parseAnswer(contentType: string, body: string): unknown {
  if (!contentType.includes("text/event-stream")) {
    return JSON.parse(body);
  }
  const data = body
    .split("\n")
    .filter((line) => line.startsWith("data:"))
    .map((line) => line.slice("data:".length).trim())
    .filter((line) => line !== "");
  const last = data.at(-1);
  if (last === undefined) {
    throw new Error(`an event stream with no data: ${body}`);
  }
  return JSON.parse(last);
}

interface ToolAnswer {
  result?: { isError?: boolean; structuredContent?: unknown };
  error?: unknown;
}

/** `tools/call` as the session; the tool's structured answer, or a throw. */
export async function callSessionTool<T>(
  sessionId: string,
  name: string,
  args: Record<string, unknown>,
): Promise<T> {
  const { url, authorization } = sessionEndpoint(sessionId);
  const response = await fetch(url, {
    method: "POST",
    headers: {
      accept: "application/json, text/event-stream",
      "content-type": "application/json",
      authorization,
      "mcp-method": "tools/call",
      "mcp-name": name,
      "mcp-protocol-version": PROTOCOL_VERSION,
    },
    body: JSON.stringify({
      jsonrpc: "2.0",
      id: 1,
      method: "tools/call",
      params: {
        // What the revision requires of every request, as the CLI sends it.
        _meta: {
          "io.modelcontextprotocol/protocolVersion": PROTOCOL_VERSION,
          "io.modelcontextprotocol/clientCapabilities": {},
          "io.modelcontextprotocol/clientInfo": {
            name: "mars-e2e",
            version: "0.0.0",
          },
        },
        name,
        arguments: args,
      },
    }),
  });
  const body = await response.text();
  if (!response.ok) {
    throw new Error(`${name}: HTTP ${String(response.status)}: ${body}`);
  }
  const answer = parseAnswer(
    response.headers.get("content-type") ?? "",
    body,
  ) as ToolAnswer;
  if (
    answer.error !== undefined ||
    answer.result === undefined ||
    answer.result.isError === true ||
    answer.result.structuredContent === undefined
  ) {
    throw new Error(`${name} failed: ${body}`);
  }
  return answer.result.structuredContent as T;
}

/** MCP `create_task` as the session, which records it as the task's author. */
export async function createTaskAsSession(
  sessionId: string,
  title: string,
): Promise<Task> {
  const { task } = await callSessionTool<{ task: Task }>(
    sessionId,
    "create_task",
    { title },
  );
  return task;
}
