// Reading a virtualised transcript from a test.
//
// `Transcript` renders only the rows in view (`SPEC.md`, "Frontend" →
// "Transcript rendering"), so a row from earlier in a turn is not merely
// off-screen: it is not in the DOM at all, and no Playwright locator can wait
// for it. The three helpers here are what a scenario asserting on transcript
// content needs — pin to the tail, scroll back until a row mounts, and count
// the whole list — and they lived in four copies before this module.
//
// Nothing here sleeps for a fixed period. Each step waits for the virtualizer's
// own re-render, seen as a DOM mutation inside the scroll container, with a
// bounded fallback for the case where the scroll changed nothing.

import { expect } from "@playwright/test";
import type { Locator, Page } from "@playwright/test";

import { TRANSCRIPT_SCROLL } from "./test-ids";

/** How long a step waits for the virtualizer's re-render before going on. */
const RENDER_BUDGET_MS = 250;

/** How far one read-back step scrolls, as a fraction of the viewport. */
const STEP_FRACTION = 0.7;

/** How many steps a read-back takes before it gives up and asserts. */
const MAX_STEPS = 60;

/** The virtualised transcript's scroll container. */
export function transcript(page: Page): Locator {
  return page.getByTestId(TRANSCRIPT_SCROLL);
}

/**
 * Waits for the next re-render inside the transcript, or `RENDER_BUDGET_MS`.
 *
 * A scroll moves the container synchronously; the virtualizer only reacts to
 * the scroll event, through React's own scheduling, so the rows for the new
 * position appear a render later. Waiting for that render — rather than for a
 * fixed period — is what keeps a read-back from scrolling straight past the
 * row it is looking for, and the budget only bounds the case where the scroll
 * mounted nothing new.
 */
async function waitForRender(page: Page): Promise<void> {
  await transcript(page).evaluate(
    (element, budget: number) =>
      new Promise<void>((resolve) => {
        const finish = (): void => {
          observer.disconnect();
          clearTimeout(timer);
          resolve();
        };
        const observer = new MutationObserver(finish);
        observer.observe(element, { childList: true, subtree: true });
        const timer = setTimeout(finish, budget);
      }),
    RENDER_BUDGET_MS,
  );
}

/**
 * Pins the transcript back to the newest row.
 *
 * Scrolling up unpins the list, and the auto-follow also gives up when a very
 * tall row (the fixture's 8 KiB tool result) is measured after the estimate it
 * was rendered at — so the view can be left showing "Jump to latest" although
 * the reader never scrolled (g4s53). Pressing that control is what a reader
 * does, and what every assertion on the newest rows rests on.
 */
export async function pinToLatest(page: Page): Promise<void> {
  const jump = page.getByRole("button", { name: /^Jump to latest/ });
  if ((await jump.count()) > 0) {
    await jump.first().click();
  }
  await transcript(page).evaluate((element) => {
    element.scrollTop = element.scrollHeight;
  });
  await waitForRender(page);
}

/**
 * Scrolls the transcript back until `target` is in the DOM and returns it.
 *
 * The read-back starts from the tail, so a row below the current position is
 * found too, and stops at the top; the final `toBeVisible` is what turns "not
 * rendered anywhere" into a failure naming the locator.
 */
export async function reveal(page: Page, target: Locator): Promise<Locator> {
  const scroller = transcript(page);
  await pinToLatest(page);

  for (let step = 0; step < MAX_STEPS; step += 1) {
    if ((await target.count()) > 0) return target;
    const atTop = await scroller.evaluate((element, fraction: number) => {
      const was = element.scrollTop;
      element.scrollTop = Math.max(0, was - element.clientHeight * fraction);
      return was === 0;
    }, STEP_FRACTION);
    if (atTop) break;
    await waitForRender(page);
  }

  await expect(target.first()).toBeVisible();
  return target;
}

/**
 * Reveals a tool row by its header button and opens it if it is folded.
 *
 * Every tool row and every subagent group starts collapsed (`SPEC.md`,
 * "Transcript rendering"), so a scenario that asserts on a row's body — a
 * command, a diff, a nested transcript — opens it first.
 */
export async function openRow(page: Page, header: Locator): Promise<Locator> {
  const button = (await reveal(page, header)).first();
  if ((await button.getAttribute("aria-expanded")) === "false") {
    await button.click();
  }
  return button;
}

/**
 * How many rows the transcript is folding, which is the identity check every
 * "no duplicates, no gaps" assertion rests on.
 *
 * The list is virtualised, so counting mounted rows would count a window, not
 * a transcript. What the virtualizer does publish is each mounted row's
 * position in the whole list (`data-index`), and the newest row is mounted
 * whenever the view is pinned to the tail — so the highest index in the DOM,
 * read while pinned, is the length of the store's `order`.
 */
export async function rowCount(page: Page): Promise<number> {
  await pinToLatest(page);
  const read = (): Promise<number> =>
    transcript(page).evaluate((element) => {
      let highest = -1;
      for (const row of element.querySelectorAll<HTMLElement>("[data-index]")) {
        const index = Number(row.dataset.index);
        if (Number.isFinite(index) && index > highest) highest = index;
      }
      return highest + 1;
    });

  // Measuring a row can mount the next one, so the count is taken once it has
  // stopped moving rather than on the first render after the scroll: each
  // re-read waits for the next render inside the list.
  let previous = -1;
  for (let step = 0; step < MAX_STEPS; step += 1) {
    const current = await read();
    if (current === previous && current > 0) return current;
    previous = current;
    await waitForRender(page);
  }
  return previous;
}
