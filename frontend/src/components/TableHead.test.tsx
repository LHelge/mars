import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import { TableHead } from "./TableHead";
import { SCROLLER, X_SCROLLER, type TableColumn } from "./tableStyles";

afterEach(cleanup);

function renderHead(columns: readonly TableColumn[]) {
  render(
    <table>
      <TableHead columns={columns} />
    </table>,
  );
}

describe("TableHead", () => {
  it("names a column by its full label when a short one is shown on a phone", () => {
    renderHead([{ label: "Orchestrator only", short: "Orch. only" }]);

    const header = screen.getByRole("columnheader", {
      name: "Orchestrator only",
    });
    // The short text is visible below `sm` and silent to a screen reader;
    // the full label is visible from `sm` up and heard at every width.
    const short = screen.getByText("Orch. only");
    expect(header.contains(short)).toBe(true);
    expect(short.getAttribute("aria-hidden")).toBe("true");
    expect(short.classList.contains("sm:hidden")).toBe(true);
    expect(
      screen
        .getByText("Orchestrator only")
        .classList.contains("max-sm:sr-only"),
    ).toBe(true);
  });

  it("renders a plain label and a screen-reader-only one as before", () => {
    renderHead([{ label: "Name" }, { label: "Actions", srOnly: true }]);

    expect(screen.getByRole("columnheader", { name: "Name" }).textContent).toBe(
      "Name",
    );
    expect(screen.getByText("Actions").classList.contains("sr-only")).toBe(
      true,
    );
  });
});

describe("SCROLLER", () => {
  // `SPEC.md`, "Frontend", Mobile layout: a long table's height cap is `sm`
  // and up only, and it always scrolls sideways.
  it("caps the height from sm up and scrolls sideways at every width", () => {
    const classes = SCROLLER.split(" ");
    expect(classes).toContain("overflow-x-auto");
    expect(classes).toContain("sm:max-h-96");
    expect(classes).not.toContain("max-h-96");
  });

  // What it scrolls it also clips: an `sr-only` header label is absolutely
  // placed, and escapes a scroller that is not its containing block.
  it("is the containing block of what it scrolls, as X_SCROLLER is", () => {
    expect(SCROLLER.split(" ")).toContain("relative");
    expect(X_SCROLLER.split(" ")).toContain("relative");
  });
});
