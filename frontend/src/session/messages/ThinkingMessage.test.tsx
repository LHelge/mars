// Thinking is agent-written prose, so it is markdown; redacted thinking has no
// text at all and is only its header (`SPEC.md`, "Transcript rendering").

import { afterEach, describe, expect, it } from "vitest";
import { cleanup, render, screen } from "@testing-library/react";

import { ThinkingMessage } from "./ThinkingMessage";

afterEach(cleanup);

describe("ThinkingMessage", () => {
  it("renders the text as markdown", () => {
    render(
      <ThinkingMessage
        message={{
          id: "e1",
          kind: "thinking",
          text: "Weighing two routes:\n\n- read the router\n- read the tests\n",
          redacted: false,
        }}
      />,
    );

    expect(document.querySelectorAll("li")).toHaveLength(2);
    expect(screen.getByText("read the router")).toBeDefined();
  });

  it("keeps the placeholder header for redacted thinking and renders no body", () => {
    render(
      <ThinkingMessage
        message={{ id: "e2", kind: "thinking", text: "", redacted: true }}
      />,
    );

    expect(screen.getByText("Thinking (redacted by backend)")).toBeDefined();
    expect(document.querySelector("p")).toBeNull();
  });
});
