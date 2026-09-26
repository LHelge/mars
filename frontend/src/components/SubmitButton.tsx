// The app's button: the action button of every form, and every other button
// that should look like one. While `loading` it is disabled and shows a
// spinner in place of its label, keeping its width so the form does not jump.
//
// Everything a native button takes is passed through, so a disclosure toggle
// can say `aria-expanded`, an icon-only button `aria-label` or `title`, and a
// test reach it by `data-testid` — none of which used to be possible, which is
// why such buttons were hand-rolled or wrapped in a `<span title>` that the
// keyboard cannot reach. `className` is not among them: the variants below are
// the button's looks.
//
// `icon` is one entry of the icon map (`components/icons.ts`), drawn before the
// label and hidden from assistive technology: it adds recognition, never a
// name.

import type { ComponentProps, ReactNode } from "react";
import { ICON_CLASS } from "./icons";
import type { IconComponent } from "./icons";
import { Spinner } from "./Spinner";

export type SubmitButtonVariant = "primary" | "danger" | "ghost";

export interface SubmitButtonProps extends Omit<
  ComponentProps<"button">,
  "className"
> {
  /** Disables the button and replaces its label with a spinner. */
  loading?: boolean;
  children: ReactNode;
  variant?: SubmitButtonVariant;
  /** An entry of `Icon`, drawn before the label. */
  icon?: IconComponent;
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
  loading = false,
  children,
  disabled,
  variant = "primary",
  type = "submit",
  icon: IconGlyph,
  ...rest
}: SubmitButtonProps) {
  return (
    <button
      {...rest}
      type={type}
      disabled={loading || disabled}
      aria-busy={loading || undefined}
      className={`relative inline-flex items-center justify-center rounded border px-3 py-1.5 text-sm transition-opacity disabled:opacity-60 ${VARIANTS[variant]}`}
    >
      {/* The label stays in flow while loading, so the width never changes. */}
      <span
        className={`inline-flex items-center gap-1.5 ${loading ? "invisible" : ""}`}
      >
        {IconGlyph !== undefined && (
          <IconGlyph aria-hidden="true" className={ICON_CLASS} />
        )}
        {children}
      </span>
      {loading && (
        <span className="absolute inset-0 flex items-center justify-center">
          <Spinner />
        </span>
      )}
    </button>
  );
}
