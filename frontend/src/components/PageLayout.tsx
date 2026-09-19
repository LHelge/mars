// The frame for every signed-in page: one top bar, no sidebar, and a dense
// full-width content area. The nav entries are the routes of `SPEC.md`,
// "Frontend"; `/admin` is shown only to an admin, and the backend checks
// authorisation regardless — the flag here is a UI hint.

import { ArrowRightStartOnRectangleIcon } from "@heroicons/react/24/outline";
import type { ReactNode } from "react";
import { NavLink } from "react-router";
import { useAuth } from "../hooks/useAuth";

export interface PageLayoutProps {
  title?: string;
  actions?: ReactNode;
  children: ReactNode;
}

interface NavEntry {
  to: string;
  label: string;
  /** Only `/` needs it; every other entry owns its subtree. */
  end?: boolean;
  adminOnly?: boolean;
}

const NAV: NavEntry[] = [
  { to: "/", label: "Dashboard", end: true },
  { to: "/projects", label: "Projects" },
  { to: "/secrets", label: "Secrets" },
  { to: "/settings", label: "Settings" },
  { to: "/admin", label: "Admin", adminOnly: true },
];

function navClass({ isActive }: { isActive: boolean }): string {
  return [
    "shrink-0 border-b-2 px-1 py-2 text-sm whitespace-nowrap",
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
        <div className="mx-auto flex max-w-7xl items-center gap-4 px-4">
          <span className="text-console-text shrink-0 font-mono text-sm tracking-[0.2em] lowercase">
            mars
            <span className="text-console-accent">.</span>
          </span>

          {/* Below `md` the entries scroll sideways instead of wrapping. */}
          <nav
            aria-label="Main"
            className="flex min-w-0 flex-1 items-center gap-4 overflow-x-auto"
          >
            {NAV.filter((entry) => !entry.adminOnly || isAdmin).map((entry) => (
              <NavLink
                key={entry.to}
                to={entry.to}
                end={entry.end}
                className={navClass}
              >
                {entry.label}
              </NavLink>
            ))}
          </nav>

          <div className="flex shrink-0 items-center gap-2">
            {/* `user` is null until `GET /users/me` resolves at startup. */}
            <span className="text-console-muted hidden font-mono text-xs sm:inline">
              {user?.username ?? "…"}
            </span>
            <button
              type="button"
              onClick={() => {
                void logout();
              }}
              className="text-console-muted hover:text-console-text inline-flex items-center gap-1 py-2 text-xs"
            >
              <ArrowRightStartOnRectangleIcon
                aria-hidden="true"
                className="size-4"
              />
              Log out
            </button>
          </div>
        </div>
      </header>

      <main className="mx-auto w-full max-w-7xl flex-1 px-4 py-5">
        {(title !== undefined || actions !== undefined) && (
          <div className="mb-4 flex flex-wrap items-center justify-between gap-x-4 gap-y-2">
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
