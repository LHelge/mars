// `SPEC.md`, "Frontend", "Copy links": the confirmation is evidence that the
// write happened, and a refusal has to leave the URL somewhere it can still be
// copied from.

import {
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { CopyLinkButton } from "./CopyLinkButton";

// An obviously fake session id (CLAUDE.md, rule 3).
const PATH = "/sessions/00000000-0000-4000-8000-00000000abcd";

function setClipboard(value: unknown): void {
  Object.defineProperty(navigator, "clipboard", {
    configurable: true,
    value,
  });
}

beforeEach(() => {
  setClipboard(undefined);
});

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

describe("CopyLinkButton", () => {
  it("shows Link copied after the clipboard write succeeds", async () => {
    const writeText = vi.fn<(text: string) => Promise<void>>(() =>
      Promise.resolve(),
    );
    setClipboard({ writeText });

    render(<CopyLinkButton path={PATH} />);
    fireEvent.click(screen.getByRole("button", { name: "Copy link" }));

    await screen.findByRole("button", { name: "Link copied" });
    expect(writeText).toHaveBeenCalledWith(`${window.location.origin}${PATH}`);
    expect(screen.queryByLabelText("Link")).toBeNull();
  });

  it("reveals the URL in a selectable field when the clipboard refuses", async () => {
    setClipboard({
      writeText: vi.fn(() => Promise.reject(new Error("denied"))),
    });

    render(<CopyLinkButton path={PATH} />);
    fireEvent.click(screen.getByRole("button", { name: "Copy link" }));

    const field = await screen.findByLabelText("Link");
    expect(field).toHaveProperty("value", `${window.location.origin}${PATH}`);
    expect(screen.queryByRole("button", { name: "Link copied" })).toBeNull();
  });

  it("falls back the same way when there is no clipboard API at all", async () => {
    render(<CopyLinkButton path={PATH} />);
    fireEvent.click(screen.getByRole("button", { name: "Copy link" }));

    const field = await screen.findByLabelText("Link");
    expect(field).toHaveProperty("value", `${window.location.origin}${PATH}`);
  });

  it("copies the canonical path only: no query string and no fragment", async () => {
    const writeText = vi.fn<(text: string) => Promise<void>>(() =>
      Promise.resolve(),
    );
    setClipboard({ writeText });
    // The page was reached with a filter and an anchor; the link carries neither.
    window.history.replaceState({}, "", `${PATH}?tab=sessions#bottom`);

    render(<CopyLinkButton path={PATH} />);
    fireEvent.click(screen.getByRole("button", { name: "Copy link" }));

    await waitFor(() => {
      expect(writeText).toHaveBeenCalled();
    });
    const copied = writeText.mock.calls[0]?.[0];
    expect(copied).toBe(`${window.location.origin}${PATH}`);
    expect(copied).not.toContain("?");
    expect(copied).not.toContain("#");
  });
});
