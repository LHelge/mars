// The action button of every form. While `loading` it is disabled and shows a
// spinner in place of its label, keeping its width so the form does not jump.

import type { ReactNode } from "react";
import { Spinner } from "./Spinner";

export type SubmitButtonVariant = "primary" | "danger" | "ghost";

export interface SubmitButtonProps {
  loading: boolean;
  children: ReactNode;
  disabled?: boolean;
  variant?: SubmitButtonVariant;
  type?: "submit" | "button";
  onClick?: () => void;
}

const VARIANTS: Record<SubmitButtonVariant, string> = {
  primary:
    "bg-console-accent text-console-accent-contrast border-console-accent hover:opacity-90",
  danger:
    "bg-transparent text-state-failed border-state-failed hover:bg-state-failed/10",
  ghost:
    "bg-transparent text-console-muted border-console-border hover:text-console-text hover:bg-console-raised",
};

export function SubmitButton({
  loading,
  children,
  disabled,
  variant = "primary",
  type = "submit",
  onClick,
}: SubmitButtonProps) {
  return (
    <button
      type={type}
      onClick={onClick}
      disabled={loading || disabled}
      aria-busy={loading || undefined}
      className={`relative inline-flex items-center justify-center rounded border px-3 py-1.5 text-sm transition-opacity disabled:opacity-60 ${VARIANTS[variant]}`}
    >
      {/* The label stays in flow while loading, so the width never changes. */}
      <span className={loading ? "invisible" : undefined}>{children}</span>
      {loading && (
        <span className="absolute inset-0 flex items-center justify-center">
          <Spinner />
        </span>
      )}
    </button>
  );
}
