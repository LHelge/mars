// The two `/admin` tables' columns, in one module because each table's header
// and each of its rows' spanning answer row need the same list: the header is
// rendered from it and the `colSpan` is its length, so neither can drift from
// the other (`components/tableStyles.ts`, `TableColumn`).

import type { TableColumn } from "../tableStyles";

export const USER_COLUMNS: readonly TableColumn[] = [
  { label: "Username" },
  { label: "Email" },
  { label: "Admin" },
  { label: "Must change password", className: "hidden sm:table-cell" },
  { label: "Email notices", className: "hidden sm:table-cell" },
  { label: "Created", className: "hidden md:table-cell" },
  { label: "Actions", className: "pr-0 text-right", srOnly: true },
];

export const INVITE_COLUMNS: readonly TableColumn[] = [
  { label: "Email" },
  { label: "Admin" },
  { label: "Invited by", className: "hidden sm:table-cell" },
  { label: "Expires" },
  { label: "Created", className: "hidden md:table-cell" },
  { label: "Actions", className: "pr-0 text-right", srOnly: true },
];
