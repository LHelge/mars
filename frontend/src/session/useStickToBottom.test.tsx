// Auto-follow, driven through a harness that gives jsdom the scroll geometry
// it otherwise has none of (`scrollHeight`, `clientHeight` and `scrollTop` are
// all 0 in a document that is never laid out).

import { afterEach, beforeAll, describe, expect, it } from "vitest";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { useRef } from "react";

import { useStickToBottom } from "./useStickToBottom";

interface HarnessProps {
  order: string[];
  tailLength: number;
}

function Harness({ order, tailLength }: HarnessProps) {
  const scrollRef = useRef<HTMLDivElement | null>(null);
  const stick = useStickToBottom(scrollRef, { order, tailLength });
  return (
    <div ref={scrollRef} data-testid="scroller" onScroll={stick.onScroll}>
      <span data-testid="pinned">{String(stick.pinned)}</span>
      <span data-testid="count">{stick.newCount}</span>
      <button type="button" onClick={stick.jumpToLatest}>
        Jump to latest
      </button>
    </div>
  );
}

/** Gives the harness element a scrollable box of `height` inside `total`. */
function setGeometry(element: HTMLElement, total: number, top: number) {
  Object.defineProperty(element, "scrollHeight", {
    configurable: true,
    value: total,
  });
  Object.defineProperty(element, "clientHeight", {
    configurable: true,
    value: 600,
  });
  Object.defineProperty(element, "scrollTop", {
    configurable: true,
    writable: true,
    value: top,
  });
}

function pinned(): string {
  return screen.getByTestId("pinned").textContent ?? "";
}

function count(): string {
  return screen.getByTestId("count").textContent ?? "";
}

beforeAll(() => {
  (globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT =
    true;
});

afterEach(cleanup);

describe("useStickToBottom", () => {
  it("follows the tail while the view is near the bottom", () => {
    const { rerender } = render(<Harness order={["a"]} tailLength={1} />);
    const scroller = screen.getByTestId("scroller");
    // 1000 tall, 600 visible, scrolled to 380: 20px from the bottom.
    setGeometry(scroller, 1000, 380);
    fireEvent.scroll(scroller);
    expect(pinned()).toBe("true");

    rerender(<Harness order={["a", "b"]} tailLength={1} />);

    expect(scroller.scrollTop).toBe(1000);
    expect(count()).toBe("0");
  });

  it("counts appended messages once the user scrolls up", () => {
    const { rerender } = render(<Harness order={["a"]} tailLength={1} />);
    const scroller = screen.getByTestId("scroller");
    setGeometry(scroller, 1000, 0);
    fireEvent.scroll(scroller);
    expect(pinned()).toBe("false");

    rerender(<Harness order={["a", "b"]} tailLength={1} />);
    rerender(<Harness order={["a", "b", "c"]} tailLength={1} />);
    expect(count()).toBe("2");

    // A prepended history page grows the list at the front; it is not new.
    rerender(<Harness order={["older", "a", "b", "c"]} tailLength={1} />);
    expect(count()).toBe("2");
    expect(scroller.scrollTop).toBe(0);
  });

  it("re-pins and clears the count on Jump to latest", () => {
    const { rerender } = render(<Harness order={["a"]} tailLength={1} />);
    const scroller = screen.getByTestId("scroller");
    setGeometry(scroller, 1000, 0);
    fireEvent.scroll(scroller);
    rerender(<Harness order={["a", "b"]} tailLength={1} />);
    expect(count()).toBe("1");

    fireEvent.click(screen.getByRole("button", { name: "Jump to latest" }));

    expect(pinned()).toBe("true");
    expect(count()).toBe("0");
    expect(scroller.scrollTop).toBe(1000);
  });

  it("re-pins as streaming text grows the tail message", () => {
    const { rerender } = render(<Harness order={["a"]} tailLength={3} />);
    const scroller = screen.getByTestId("scroller");
    setGeometry(scroller, 1000, 390);
    fireEvent.scroll(scroller);

    // No new message, only more text in the one already there.
    rerender(<Harness order={["a"]} tailLength={40} />);

    expect(scroller.scrollTop).toBe(1000);
  });
});
