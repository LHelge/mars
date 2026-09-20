import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import { ConflictList } from "./ConflictList";

afterEach(cleanup);

describe("ConflictList", () => {
  it("names every conflicting path", () => {
    render(<ConflictList paths={["src/main.rs", "docs/data-model.md"]} />);

    expect(screen.getByText("Conflicts in:")).toBeTruthy();
    const paths = screen.getAllByRole("listitem").map((li) => li.textContent);
    expect(paths).toEqual(["src/main.rs", "docs/data-model.md"]);
  });

  it("shows the server's sentence under the paths", () => {
    render(<ConflictList paths={["a.txt"]} message="merge failed" />);

    expect(screen.getByRole("alert").textContent).toContain("merge failed");
  });
});
