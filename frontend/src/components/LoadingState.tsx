// The placeholder shown while a view's first read is in flight.

import { Spinner } from "./Spinner";

export interface LoadingStateProps {
  label?: string;
}

export function LoadingState({ label = "Loading" }: LoadingStateProps) {
  return (
    <div
      role="status"
      className="text-console-muted flex items-center gap-2 px-1 py-6 text-sm"
    >
      <Spinner />
      <span>{label}</span>
    </div>
  );
}
