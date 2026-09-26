// The one spinner in the kit, shared by `SubmitButton` and `LoadingState`.
// Internal: pages compose `SubmitButton` and `LoadingState` rather than this.

export interface SpinnerProps {
  /** Tailwind size classes, e.g. `size-4`. */
  className?: string;
}

export function Spinner({ className = "size-4" }: SpinnerProps) {
  return (
    <svg
      aria-hidden="true"
      viewBox="0 0 16 16"
      fill="none"
      className={`animate-spin ${className}`}
    >
      <circle
        cx="8"
        cy="8"
        r="6"
        stroke="currentColor"
        strokeOpacity="0.25"
        strokeWidth="2"
      />
      <path
        d="M14 8a6 6 0 0 0-6-6"
        stroke="currentColor"
        strokeWidth="2"
        strokeLinecap="round"
      />
    </svg>
  );
}
