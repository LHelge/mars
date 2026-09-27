// The board half of `SPEC.md`, "Frontend", "Task-board search": the field, the
// cards it hides, the columns it keeps, and the `No matching tasks` state kept
// apart from loading, a failed load and an empty project.
//
// The board reads only the store, so these drive the store directly and assert
// that typing never reaches the services.

import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import {
  act,
  cleanup,
  fireEvent,
  render,
  screen,
  within,
} from "@testing-library/react";
import { MemoryRouter } from "react-router";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { listTasks } from "../services/tasks";
import { listTaskStates } from "../services/taskStates";
import { getUser } from "../services/users";
import { DELAYED_FLAG_MS } from "../utils/useDelayedFlag";
import type { Task, TaskState } from "../types";
import { TaskBoard } from "./TaskBoard";
import { emptyTaskBoardState, taskSnapshot, useTaskStore } from "./taskStore";

vi.mock("../services/tasks", () => ({ listTasks: vi.fn() }));
vi.mock("../services/taskStates", () => ({ listTaskStates: vi.fn() }));
vi.mock("../services/users", () => ({ getUser: vi.fn() }));

// Obviously fake fixture values (`CLAUDE.md`, rule 3).
const PROJECT_ID = "00000000-0000-0000-0000-0000000000a1";

function state(name: string, position: number): TaskState {
  return {
    id: `state-${name}`,
    project_id: PROJECT_ID,
    name,
    kind: "queue",
    position,
    auto_merge: false,
    conflict_state: null,
    created_at: "2026-03-01T09:00:00Z",
  };
}

function task(number: number, title: string, stateName: string): Task {
  return {
    id: `task-${String(number)}`,
    project_id: PROJECT_ID,
    number,
    title,
    description: null,
    state: stateName,
    priority: 2,
    blocked: false,
    labels: [],
    parent_id: null,
    assignee_user_id: null,
    lease_holder_session_id: null,
    lease_since: null,
    attempts: 0,
    rounds: 0,
    needs_human_reason: null,
    handoff: null,
    depends_on: [],
    blocks: [],
    created_by_session_id: null,
    created_at: "2026-03-01T09:00:00Z",
    updated_at: "2026-03-01T09:00:00Z",
    closed_at: null,
  };
}

const STATES = [state("backlog", 0), state("ready", 1)];
const TASKS = [
  task(42, "Fix login redirect", "ready"),
  task(142, "Rotate the deploy key", "backlog"),
];

function loadedBoard(tasks: Task[] = TASKS): void {
  useTaskStore.setState({
    ...emptyTaskBoardState(),
    ...taskSnapshot(STATES, tasks),
    projectId: PROJECT_ID,
    loaded: true,
    stream: "live",
  });
}

function show(): void {
  render(
    <QueryClientProvider client={new QueryClient()}>
      <MemoryRouter>
        <TaskBoard projectId={PROJECT_ID} />
      </MemoryRouter>
    </QueryClientProvider>,
  );
}

function field(): HTMLInputElement {
  return screen.getByRole("searchbox", { name: "Search tasks" });
}

beforeEach(() => {
  vi.mocked(getUser).mockResolvedValue({
    id: "user-1",
    username: "someone",
    email: "someone@example.test",
    admin: false,
    must_change_password: false,
    notify_email: true,
    created_at: "2026-03-01T09:00:00Z",
  });
});

afterEach(() => {
  cleanup();
  vi.clearAllMocks();
  useTaskStore.setState(emptyTaskBoardState());
});

describe("TaskBoard search", () => {
  it("offers a labelled field with the specified placeholder", () => {
    loadedBoard();
    show();

    const input = field();
    expect(input.placeholder).toBe("Search title or #number");
    expect(screen.getByText("Search").getAttribute("for")).toBe(input.id);
  });

  it("hides the cards that do not match and keeps every column", () => {
    loadedBoard();
    show();

    fireEvent.change(field(), { target: { value: "login" } });

    expect(screen.getByText("Fix login redirect")).toBeTruthy();
    expect(screen.queryByText("Rotate the deploy key")).toBeNull();
    // Both headings stay; the filter narrows the board, it does not reshape it.
    expect(screen.getByRole("heading", { name: "backlog" })).toBeTruthy();
    expect(screen.getByRole("heading", { name: "ready" })).toBeTruthy();
  });

  it("matches the exact number for #42", () => {
    loadedBoard();
    show();

    fireEvent.change(field(), { target: { value: "#42" } });

    expect(screen.getByText("Fix login redirect")).toBeTruthy();
    expect(screen.queryByText("Rotate the deploy key")).toBeNull();
  });

  it("issues no reads while typing", () => {
    loadedBoard();
    show();

    fireEvent.change(field(), { target: { value: "log" } });
    fireEvent.change(field(), { target: { value: "login" } });

    expect(vi.mocked(listTasks)).not.toHaveBeenCalled();
    expect(vi.mocked(listTaskStates)).not.toHaveBeenCalled();
  });

  it("shows No matching tasks with a clear action, and restores on clear", () => {
    loadedBoard();
    show();

    fireEvent.change(field(), { target: { value: "nothing here" } });
    const empty = screen.getByText("No matching tasks").closest("div");
    expect(empty).not.toBeNull();

    fireEvent.click(
      within(empty as HTMLElement).getByRole("button", {
        name: "Clear search",
      }),
    );

    expect(screen.queryByText("No matching tasks")).toBeNull();
    expect(screen.getByText("Rotate the deploy key")).toBeTruthy();
    expect(document.activeElement).toBe(field());
  });

  it("keeps the empty project's own state when nothing is typed", () => {
    loadedBoard([]);
    show();

    expect(screen.getByText("No tasks yet")).toBeTruthy();
    expect(screen.queryByText("No matching tasks")).toBeNull();
  });

  it("says nothing about matches before the first load", () => {
    useTaskStore.setState({ ...emptyTaskBoardState(), projectId: PROJECT_ID });
    show();

    fireEvent.change(field(), { target: { value: "login" } });

    expect(screen.queryByText("No matching tasks")).toBeNull();
    expect(screen.getByText("Loading the task board")).toBeTruthy();
  });

  it("says nothing about matches when a load failed with no snapshot", () => {
    useTaskStore.setState({
      ...emptyTaskBoardState(),
      projectId: PROJECT_ID,
      error: "the board could not be read",
    });
    show();

    fireEvent.change(field(), { target: { value: "login" } });

    expect(screen.queryByText("No matching tasks")).toBeNull();
    expect(screen.getByText("the board could not be read")).toBeTruthy();
  });
});

// `SPEC.md`, "Frontend", "Board refresh ordering": one `role="status"`, and a
// refresh is normally over before it could be read, so only a stream that is
// not live says anything at once.
describe("the board's status marker", () => {
  afterEach(() => {
    vi.useRealTimers();
  });

  it("says nothing about a refresh that is over quickly", () => {
    vi.useFakeTimers();
    loadedBoard();
    useTaskStore.setState({ loading: true });
    show();

    expect(screen.queryByRole("status")).toBeNull();

    act(() => {
      vi.advanceTimersByTime(DELAYED_FLAG_MS - 1);
      useTaskStore.setState({ loading: false });
    });
    act(() => {
      vi.advanceTimersByTime(DELAYED_FLAG_MS);
    });

    expect(screen.queryByRole("status")).toBeNull();
  });

  it("says Refreshing once a refresh has taken a while", () => {
    vi.useFakeTimers();
    loadedBoard();
    useTaskStore.setState({ loading: true });
    show();

    act(() => {
      vi.advanceTimersByTime(DELAYED_FLAG_MS);
    });

    expect(screen.getByRole("status").textContent).toBe("Refreshing");
  });

  it("says Reconnecting without waiting", () => {
    loadedBoard();
    useTaskStore.setState({ stream: "reconnecting" });
    show();

    expect(screen.getByRole("status").textContent).toBe("Reconnecting");
  });
});

// `SPEC.md`, "Frontend", "Mobile layout": below `sm` a row of chips above the
// strip names every column, marks the one in view and scrolls to the one
// tapped. jsdom has neither `IntersectionObserver` nor `scrollIntoView`, and
// `matchMedia` absent reads as the narrow layout, so each is stubbed here.
describe("the column picker below sm", () => {
  interface Observed {
    callback: IntersectionObserverCallback;
    options: IntersectionObserverInit | undefined;
    targets: Element[];
  }
  let observers: Observed[];
  let scrolled: { element: Element; options: unknown }[];

  beforeEach(() => {
    observers = [];
    scrolled = [];
    vi.stubGlobal(
      "IntersectionObserver",
      class {
        private readonly observed: Observed;
        constructor(
          callback: IntersectionObserverCallback,
          options?: IntersectionObserverInit,
        ) {
          this.observed = { callback, options, targets: [] };
          observers.push(this.observed);
        }
        observe(target: Element) {
          this.observed.targets.push(target);
        }
        disconnect() {
          this.observed.targets = [];
        }
      },
    );
    Element.prototype.scrollIntoView = function (
      this: Element,
      options?: unknown,
    ) {
      scrolled.push({ element: this, options });
    };
  });

  afterEach(() => {
    vi.unstubAllGlobals();
    // jsdom has no `scrollIntoView` of its own to put back.
    Reflect.deleteProperty(Element.prototype, "scrollIntoView");
  });

  function picker(): HTMLElement {
    return screen.getByRole("navigation", { name: "Board columns" });
  }

  function chip(name: RegExp): HTMLElement {
    return within(picker()).getByRole("button", { name });
  }

  /** The observer reports `name`'s column as the one in view. */
  function bringIntoView(name: string): void {
    const observed = observers.at(-1);
    const target = observed?.targets.find(
      (element) => element.getAttribute("data-testid") === `column-${name}`,
    );
    if (observed === undefined || target === undefined) {
      throw new Error(`column ${name} is not observed`);
    }
    act(() => {
      observed.callback(
        [
          {
            target,
            isIntersecting: true,
            intersectionRatio: 1,
          } as IntersectionObserverEntry,
        ],
        {} as IntersectionObserver,
      );
    });
  }

  it("names every column with its count and marks the first", () => {
    loadedBoard();
    show();

    expect(chip(/^backlog \(1\)$/).getAttribute("aria-current")).toBe("true");
    expect(chip(/^ready \(1\)$/).getAttribute("aria-current")).toBeNull();
  });

  it("observes the columns from the strip at a 0.6 threshold", () => {
    loadedBoard();
    show();

    const observed = observers.at(-1);
    expect(observed?.options?.threshold).toBe(0.6);
    expect(observed?.options?.root).toBe(
      screen.getByTestId("column-backlog").parentElement,
    );
    expect(
      observed?.targets.map((element) => element.getAttribute("data-testid")),
    ).toEqual(["column-backlog", "column-ready"]);
  });

  it("scrolls the strip to a tapped column and marks it", () => {
    loadedBoard();
    show();

    fireEvent.click(chip(/^ready/));

    expect(scrolled).toHaveLength(1);
    expect(scrolled[0]?.element).toBe(screen.getByTestId("column-ready"));
    expect(scrolled[0]?.options).toMatchObject({ inline: "start" });
    expect(chip(/^ready/).getAttribute("aria-current")).toBe("true");
    expect(chip(/^backlog/).getAttribute("aria-current")).toBeNull();
  });

  it("follows a swipe through the observer, scrolling nothing itself", () => {
    loadedBoard();
    show();

    bringIntoView("ready");

    expect(chip(/^ready/).getAttribute("aria-current")).toBe("true");
    expect(scrolled).toHaveLength(0);
  });

  it("is still there for a project with one column", () => {
    useTaskStore.setState({
      ...emptyTaskBoardState(),
      ...taskSnapshot([state("backlog", 0)], []),
      projectId: PROJECT_ID,
      loaded: true,
      stream: "live",
    });
    show();

    expect(within(picker()).getAllByRole("button")).toHaveLength(1);
    expect(chip(/^backlog \(0\)$/).getAttribute("aria-current")).toBe("true");
  });

  it("is not rendered at sm and wider", () => {
    vi.stubGlobal("matchMedia", (query: string) => ({
      matches: true,
      media: query,
      addEventListener: () => undefined,
      removeEventListener: () => undefined,
    }));
    loadedBoard();
    show();

    expect(
      screen.queryByRole("navigation", { name: "Board columns" }),
    ).toBeNull();
    expect(screen.getByTestId("column-backlog")).toBeTruthy();
  });
});
