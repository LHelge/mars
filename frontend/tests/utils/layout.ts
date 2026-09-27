// The one spelling of "nothing on this page is wider than the screen"
// (`SPEC.md`, "Frontend", Mobile layout), for every phone scenario.

import { expect } from "@playwright/test";
import type { Page } from "@playwright/test";

/** How far past the viewport's right edge a box may reach: sub-pixel rounding. */
const TOLERANCE_PX = 1;

/**
 * Asserts that `page` does not scroll sideways and that nothing inside its
 * `main` reaches past the viewport's right edge.
 *
 * The boxes come first, because a failure that names them says what to fix:
 * each offender is listed with its tag, test id or label, the start of its
 * text and how far it reaches. A box inside a scroller of its own (an
 * ancestor below `main` whose `overflow-x` is not `visible`) is that
 * scroller's business — a table in its box, the task board's column strip —
 * as long as the scroller itself fits, and the scroller is one more element
 * of `main`. An absolutely placed box escapes a scroller that does not hold
 * its containing block, as an `sr-only` label does, so it is excused only by
 * one that does.
 *
 * Then the page as a whole, what a user feels: the document no wider than the
 * viewport the scenario set, and the layout viewport no wider either. The
 * width compared against is Playwright's, not `window.innerWidth`, because a
 * phone browser zooms a page wider than the screen out until it fits, and
 * then `innerWidth` is the page's width rather than the screen's.
 *
 * `label` names the route or state in the failure message.
 */
export async function expectNoHorizontalOverflow(
  page: Page,
  label = page.url(),
): Promise<void> {
  const width = page.viewportSize()?.width;
  expect(width, `${label}: the page has a fixed viewport`).toBeDefined();
  const limit = width ?? 0;

  const report = await page.evaluate(
    ([tolerance, right]) => {
      function describe(el: Element): string {
        const testId = el.getAttribute("data-testid");
        const aria = el.getAttribute("aria-label");
        const text = (el.textContent ?? "").trim().replace(/\s+/g, " ");
        return [
          el.tagName.toLowerCase(),
          testId === null ? "" : `[data-testid=${testId}]`,
          aria === null ? "" : `[aria-label=${aria}]`,
          text === "" ? "" : ` "${text.slice(0, 40)}"`,
        ].join("");
      }

      // Only a scroller inside `main` excuses a box: `main` itself, or
      // anything above it, clipping one would cut content off the screen
      // rather than scroll it.
      function clippedInside(el: Element, main: Element): boolean {
        const own = getComputedStyle(el).position;
        if (own === "fixed") return false;
        let escaping = own === "absolute";
        for (
          let ancestor = el.parentElement;
          ancestor !== null && ancestor !== main;
          ancestor = ancestor.parentElement
        ) {
          const style = getComputedStyle(ancestor);
          if (escaping && style.position !== "static") escaping = false;
          if (!escaping && style.overflowX !== "visible") return true;
        }
        return false;
      }

      const offenders: string[] = [];
      for (const main of document.querySelectorAll("main")) {
        for (const el of main.querySelectorAll("*")) {
          const rect = el.getBoundingClientRect();
          if (rect.width === 0 || rect.height === 0) continue;
          if (rect.right <= right + tolerance) continue;
          if (clippedInside(el, main)) continue;
          offenders.push(
            `${describe(el)} reaches ${String(Math.round(rect.right))} px of ${String(right)}`,
          );
        }
      }

      return {
        scrollWidth: document.documentElement.scrollWidth,
        innerWidth: window.innerWidth,
        mains: document.querySelectorAll("main").length,
        offenders,
      };
    },
    [TOLERANCE_PX, limit] as const,
  );

  expect(report.mains, `${label}: the page has a main`).toBeGreaterThan(0);
  expect(
    report.offenders,
    `${label}: boxes past the viewport's right edge`,
  ).toEqual([]);
  expect(
    report.scrollWidth,
    `${label}: the document is ${String(report.scrollWidth)} px wide in a ${String(limit)} px viewport`,
  ).toBeLessThanOrEqual(limit);
  // The same pixel of slack as the boxes: a layout viewport rounded up.
  expect(
    report.innerWidth,
    `${label}: the browser zoomed out to fit a page wider than the screen`,
  ).toBeLessThanOrEqual(limit + TOLERANCE_PX);
}
