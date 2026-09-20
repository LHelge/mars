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
  /** What a virtualised list reports once its rows have been measured. */
  contentHeight?: number;
  /** Called inside the scroll handler with the rendered and the live flag. */
  onHandled?: (rendered: boolean, live: boolean) => void;
}

function Harness({
  order,
  tailLength,
  contentHeight,
  onHandled,
}: HarnessProps) {
  const scrollRef = useRef<HTMLDivElement | null>(null);
  const stick = useStickToBottom(scrollRef, {
    order,
    tailLength,
    contentHeight,
  });
  return (
    <div
      ref={scrollRef}
      data-testid="scroller"
      onScroll={() => {
        stick.onScroll();
        onHandled?.(stick.pinned, stick.isPinned());
      }}
    >
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
    // The reader starts on the tail and pulls the viewport up to the top, which
    // is the movement the hook reads as "stop following".
    setGeometry(scroller, 1000, 400);
    fireEvent.scroll(scroller);
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
    setGeometry(scroller, 1000, 400);
    fireEvent.scroll(scroller);
    setGeometry(scroller, 1000, 0);
    fireEvent.scroll(scroller);
    rerender(<Harness order={["a", "b"]} tailLength={1} />);
    expect(count()).toBe("1");

    fireEvent.click(screen.getByRole("button", { name: "Jump to latest" }));

    expect(pinned()).toBe("true");
    expect(count()).toBe("0");
    expect(scroller.scrollTop).toBe(1000);
  });

  // The virtualizer renders rows at an estimate and corrects itself a commit
  // later: the message list is unchanged, only the content it occupies grew.
  it("re-pins when measurement corrects the content height", () => {
    const { rerender } = render(
      <Harness order={["a"]} tailLength={1} contentHeight={200} />,
    );
    const scroller = screen.getByTestId("scroller");
    setGeometry(scroller, 1000, 380);
    fireEvent.scroll(scroller);
    expect(pinned()).toBe("true");

    setGeometry(scroller, 2400, 380);
    rerender(<Harness order={["a"]} tailLength={1} contentHeight={2000} />);

    expect(scroller.scrollTop).toBe(2400);
  });

  // The virtualizer renders a tall row at the estimate and corrects the
  // scroller from its own measurement callback, outside React's commit: the
  // content below the viewport grows by thousands of pixels while the reader
  // has not touched anything, and the `scroll` that correction fires reports a
  // distance from the bottom far past `NEAR_BOTTOM_PX`.
  it("stays pinned when a measurement moves the scroller far from the bottom", () => {
    render(<Harness order={["a"]} tailLength={1} contentHeight={200} />);
    const scroller = screen.getByTestId("scroller");
    setGeometry(scroller, 1000, 400);
    fireEvent.scroll(scroller);
    expect(pinned()).toBe("true");

    // The 8 KiB tool result is measured: the same messages at the same scroll
    // position, with 8000 more pixels of content underneath.
    setGeometry(scroller, 9000, 400);
    fireEvent.scroll(scroller);

    expect(pinned()).toBe("true");
    expect(scroller.scrollTop).toBe(9000);
  });

  // The other half of the same rule: moving the viewport upwards is the one
  // thing only a reader does.
  it("unpins when the reader moves the viewport up", () => {
    render(<Harness order={["a"]} tailLength={1} contentHeight={200} />);
    const scroller = screen.getByTestId("scroller");
    setGeometry(scroller, 1000, 400);
    fireEvent.scroll(scroller);
    expect(pinned()).toBe("true");

    setGeometry(scroller, 1000, 100);
    fireEvent.scroll(scroller);

    expect(pinned()).toBe("false");
    expect(scroller.scrollTop).toBe(100);
  });

  // The transcript anchors a history request inside its scroll handler, where
  // `pinned` is still the value from the render before the gesture.
  it("answers the live flag from inside the scroll handler", () => {
    const seen: [boolean, boolean][] = [];
    render(
      <Harness
        order={["a"]}
        tailLength={1}
        onHandled={(rendered, live) => {
          seen.push([rendered, live]);
        }}
      />,
    );
    const scroller = screen.getByTestId("scroller");

    // The reader jumps from the tail to the top in one gesture.
    setGeometry(scroller, 1000, 400);
    fireEvent.scroll(scroller);
    setGeometry(scroller, 1000, 0);
    fireEvent.scroll(scroller);

    // Still on the tail for the first event, and the second one reports the
    // flag it has just flipped while the render still shows the old value.
    expect(seen).toEqual([
      [true, true],
      [true, false],
    ]);
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
