// A one-message banner. An error is announced immediately (`role="alert"`);
// everything else is polite (`role="status"`).

import type { ReactNode } from "react";

import { TAP } from "./fieldStyles";
import { Icon, ICON_CLASS } from "./icons";

export type AlertKind = "error" | "success" | "info" | "warning";

export interface AlertProps {
  kind: AlertKind;
  children: ReactNode;
  onDismiss?: () => void;
}

const KINDS: Record<AlertKind, string> = {
  error: "border-state-failed/60 text-state-failed",
  success: "border-state-running/60 text-state-running",
  warning: "border-state-parked/60 text-state-parked",
  info: "border-console-border text-console-muted",
};

export function Alert({ kind, children, onDismiss }: AlertProps) {
  return (
    <div
      role={kind === "error" ? "alert" : "status"}
      className={`bg-console-surface flex items-start gap-2 rounded border px-3 py-2 text-sm ${KINDS[kind]}`}
    >
      <div className="min-w-0 flex-1">{children}</div>
      {onDismiss && (
        <button
          type="button"
          onClick={onDismiss}
          aria-label="Dismiss"
          className={`text-console-muted hover:text-console-text -m-1 shrink-0 p-1 ${TAP}`}
        >
          <Icon.close aria-hidden="true" className={ICON_CLASS} />
        </button>
      )}
    </div>
  );
}
