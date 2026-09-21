// Thinking is agent-written prose, so it is markdown; redacted thinking has no
// text at all and is only its header (`SPEC.md`, "Transcript rendering").

import { afterEach, describe, expect, it } from "vitest";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";

import { ThinkingMessage } from "./ThinkingMessage";

afterEach(cleanup);

describe("ThinkingMessage", () => {
  it("renders the text as markdown once it is opened", () => {
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

    // Folded, the body does not exist at all: the markdown is parsed when the
    // reader asks for it.
    expect(document.querySelectorAll("li")).toHaveLength(0);

    fireEvent.click(screen.getByRole("button", { name: "Thinking" }));

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
