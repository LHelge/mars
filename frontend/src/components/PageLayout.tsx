// The frame for every signed-in page: one top bar, no sidebar, and a dense
// full-width content area. The nav entries are the routes of `SPEC.md`,
// "Frontend"; `/admin` is shown only to an admin, and the backend checks
// authorisation regardless — the flag here is a UI hint. `/help` is not a place
// work happens, so it sits in the right-hand cluster beside the account rather
// than among the work areas.
//
// Below `sm` the same entries stay, smaller: every link is its icon with the
// label kept as its accessible name (`max-sm:sr-only`), the account cluster is
// two icons, and nothing hides behind a menu (`SPEC.md`, "Frontend", Mobile
// layout).

import type { ReactNode } from "react";
import { NavLink } from "react-router";
import { useAuth } from "../hooks/useAuth";
import { Icon, ICON_CLASS } from "./icons";
import type { IconComponent } from "./icons";
import { TAP } from "./fieldStyles";

export interface PageLayoutProps {
  title?: string;
  actions?: ReactNode;
  children: ReactNode;
}

interface NavEntry {
  to: string;
  label: string;
  icon: IconComponent;
  /** Only `/` needs it; every other entry owns its subtree. */
  end?: boolean;
  adminOnly?: boolean;
}

const NAV: NavEntry[] = [
  { to: "/", label: "Dashboard", icon: Icon.dashboard, end: true },
  { to: "/projects", label: "Projects", icon: Icon.projects },
  { to: "/secrets", label: "Secrets", icon: Icon.secrets },
  { to: "/settings", label: "Settings", icon: Icon.settings },
  { to: "/admin", label: "Admin", icon: Icon.admin, adminOnly: true },
];

function navClass({ isActive }: { isActive: boolean }): string {
  return [
    // Icon-only below `sm`: centred in its 44 px touch box, and 3 px more
    // padding each way so the 14 px icon stands where the 20 px line of text
    // did and a narrow window's header keeps its height.
    `inline-flex shrink-0 items-center justify-center gap-1.5 border-b-2 px-1 py-2 text-sm whitespace-nowrap max-sm:py-[0.6875rem] ${TAP}`,
    isActive
      ? "border-console-accent text-console-text"
      : "border-transparent text-console-muted hover:text-console-text",
  ].join(" ");
}

export function PageLayout({ title, actions, children }: PageLayoutProps) {
  const { user, isAdmin, logout } = useAuth();

  return (
    <div className="flex min-h-full flex-col">
      <header className="border-console-border bg-console-surface border-b">
        {/* The row scrolls only as a safety net, for a screen narrower than
            the phones `SPEC.md`, "Frontend", Mobile layout, is written for —
            an administrator's seven touch boxes below about 400 px. It is
            `relative` so that it clips what it scrolls: the labels' `sr-only`
            spans are absolutely placed, and without a positioned ancestor
            inside the row one past its edge widened the whole page. */}
        <div className="relative mx-auto flex max-w-7xl items-center gap-4 overflow-x-auto px-4 max-sm:gap-2">
          <span className="text-console-text shrink-0 font-mono text-sm tracking-[0.2em] lowercase max-sm:tracking-normal">
            mars
            <span className="text-console-accent">.</span>
          </span>

          {/* From `sm` the labelled entries scroll sideways when the row is
              short of room. Below `sm` they are icons whose 44 px touch boxes
              sit edge to edge and share the spare width, and the nav is never
              narrower than they are: a phone sees all of them, and only a
              screen too narrow for the row scrolls it as a whole. */}
          <nav
            aria-label="Main"
            className="flex min-w-0 flex-1 items-center gap-4 overflow-x-auto max-sm:min-w-fit max-sm:justify-between max-sm:gap-0"
          >
            {NAV.filter((entry) => !entry.adminOnly || isAdmin).map((entry) => (
              <NavLink
                key={entry.to}
                to={entry.to}
                end={entry.end}
                className={navClass}
              >
                <entry.icon aria-hidden="true" className={ICON_CLASS} />
                {/* The accessible name at every width, visible from `sm`. */}
                <span className="max-sm:sr-only">{entry.label}</span>
              </NavLink>
            ))}
          </nav>

          <div className="flex shrink-0 items-center gap-3 max-sm:gap-0">
            <NavLink
              to="/help"
              className={({ isActive }) =>
                [
                  `inline-flex items-center justify-center gap-1 py-2 text-xs ${TAP}`,
                  isActive
                    ? "text-console-text"
                    : "text-console-muted hover:text-console-text",
                ].join(" ")
              }
            >
              <Icon.help aria-hidden="true" className={ICON_CLASS} />
              {/* Icon-only below `sm`, where the work areas need the room. */}
              <span className="max-sm:sr-only">Help</span>
            </NavLink>
            {/* `user` is null until `GET /users/me` resolves at startup. */}
            <span className="text-console-muted hidden font-mono text-xs sm:inline">
              {user?.username ?? "…"}
            </span>
            <button
              type="button"
              onClick={() => {
                void logout();
              }}
              className={`text-console-muted hover:text-console-text inline-flex items-center justify-center gap-1 py-2 text-xs ${TAP}`}
            >
              <Icon.logout aria-hidden="true" className={ICON_CLASS} />
              {/* Icon-only below `sm`; the text stays its accessible name. */}
              <span className="max-sm:sr-only">Log out</span>
            </button>
          </div>
        </div>
      </header>

      {/* Focusable by script only: it is where focus lands when something
          modal closes and the element that opened it is gone (`src/tasks/
          TaskDetail.tsx`), rather than at the top of the document. */}
      <main
        tabIndex={-1}
        className="mx-auto w-full max-w-7xl flex-1 px-4 py-5 outline-none"
      >
        {(title !== undefined || actions !== undefined) && (
          // With actions the row stacks below `sm`, the actions under the title
          // rather than squeezed beside it.
          <div
            className={`mb-4 flex flex-wrap items-center justify-between gap-x-4 gap-y-2 ${actions ? "max-sm:flex-col max-sm:items-start" : ""}`}
          >
            {title && (
              <h1 className="text-console-text min-w-0 text-base font-semibold tracking-tight">
                {title}
              </h1>
            )}
            {actions && (
              <div className="flex shrink-0 items-center gap-2">{actions}</div>
            )}
          </div>
        )}
        {children}
      </main>
    </div>
  );
}
