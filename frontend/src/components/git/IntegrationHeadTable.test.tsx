import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { IntegrationHeadTable } from "./IntegrationHeadTable";
import type { IntegrationHead } from "./integrationHeads";

afterEach(cleanup);

const HEADS: IntegrationHead[] = [
  { name: "main", commit: "a".repeat(40), isDefault: true, upstream: null },
  { name: "release", commit: "b".repeat(40), isDefault: false, upstream: null },
];

describe("IntegrationHeadTable", () => {
  it("offers Push… on every head and reports which one was asked for", () => {
    const onToggle = vi.fn();
    render(
      <IntegrationHeadTable
        heads={HEADS}
        open={null}
        onToggle={onToggle}
        renderForm={() => <p>form</p>}
        disabled={false}
      />,
    );

    const buttons = screen.getAllByRole("button", { name: "Push…" });
    expect(buttons).toHaveLength(2);
    expect(screen.queryByText("form")).toBeNull();

    const release = buttons[1];
    if (release === undefined) throw new Error("no second button");
    fireEvent.click(release);
    expect(onToggle).toHaveBeenCalledWith("release");
  });

  it("renders the open head's form under its row, and only that one", () => {
    const renderForm = vi.fn((head: IntegrationHead) => (
      <p>{`form for ${head.name}`}</p>
    ));
    render(
      <IntegrationHeadTable
        heads={HEADS}
        open="main"
        onToggle={() => undefined}
        renderForm={renderForm}
        disabled={false}
      />,
    );

    expect(screen.getByText("form for main")).toBeTruthy();
    expect(screen.queryByText("form for release")).toBeNull();
    const [main, release] = screen.getAllByRole("button", { name: "Push…" });
    expect(main?.getAttribute("aria-expanded")).toBe("true");
    expect(release?.getAttribute("aria-expanded")).toBe("false");
  });

  it("disables every head's action while a git form is in flight", () => {
    render(
      <IntegrationHeadTable
        heads={HEADS}
        open={null}
        onToggle={() => undefined}
        renderForm={() => null}
        disabled
      />,
    );

    for (const button of screen.getAllByRole("button", { name: "Push…" })) {
      expect((button as HTMLButtonElement).disabled).toBe(true);
    }
  });
});
