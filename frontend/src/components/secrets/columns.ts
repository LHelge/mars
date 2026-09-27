// The secrets table's columns. The header is `SecretsManager`'s and the rows
// that span the table are `SecretRow`'s, in another file: the list lives here
// so the span is the header's length and not a number copied across the folder
// (`components/tableStyles.ts`, `TableColumn`).

import type { TableColumn } from "../tableStyles";

export const SECRET_COLUMNS: readonly TableColumn[] = [
  { label: "Name" },
  { label: "Orchestrator only", short: "Orch. only" },
  { label: "Key", className: "hidden sm:table-cell" },
  { label: "Created by", className: "hidden lg:table-cell" },
  { label: "Created", className: "hidden md:table-cell" },
  { label: "Updated", className: "hidden md:table-cell" },
  { label: "Last used" },
  { label: "Actions", className: "pr-0 text-right" },
];
