// The frame for every page reached without a session: login, invite,
// forgot/reset password and the forced password change. One centred card, the
// product name above it, nothing else to look at.

import type { ReactNode } from "react";

export interface AuthLayoutProps {
  title: string;
  children: ReactNode;
  /** Secondary links, e.g. "Forgot your password?". */
  footer?: ReactNode;
}

export function AuthLayout({ title, children, footer }: AuthLayoutProps) {
  return (
    <main className="flex min-h-full items-center justify-center px-4 py-10">
      <div className="w-full max-w-sm">
        <p className="text-console-text mb-6 font-mono text-base tracking-[0.2em] lowercase">
          mars
          <span className="text-console-accent">.</span>
        </p>

        <div className="border-console-border bg-console-surface rounded border p-5">
          <h1 className="text-console-text mb-4 text-sm font-semibold tracking-tight">
            {title}
          </h1>
          {children}
        </div>

        {footer && (
          <div className="text-console-muted mt-4 text-xs">{footer}</div>
        )}
      </div>
    </main>
  );
}
