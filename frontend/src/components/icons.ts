// The console's one icon map (ADR 0047): every icon the UI draws is named here
// by the concept it stands for, and components import `Icon` from this file,
// never from `lucide-react` — ESLint's `no-restricted-imports` refuses the
// direct import. One map is what keeps a concept one glyph everywhere (a push
// is the same arrow in the branch table and in its form) and makes a later
// change of glyph, or of library, one edit.
//
// An icon always sits beside its words and never replaces them: it is
// `aria-hidden`, and a button that has only an icon keeps its `aria-label`.
// `ICON_CLASS` is the one size, so a row of actions reads as one row; the
// colour is the text's own (`currentColor`), and so a state icon is exactly as
// coloured as the label beside it (`CLAUDE.md`, "Frontend conventions": quiet
// colour reserved for state).
//
// Each icon is a named import, so the bundle carries the ones mapped here and
// not the library. `PageLayout` and `StatusBadge` are on the first-paint path
// and read this map, so every icon in it is in the entry chunk: a few hundred
// bytes each, measured by `scripts/check-entry-chunk.mjs`.

import {
  Activity,
  ArrowUpFromLine,
  Bot,
  Check,
  ChevronDown,
  ChevronRight,
  ChevronsLeft,
  ChevronsRight,
  CircleCheck,
  CirclePause,
  CircleQuestionMark,
  CircleStop,
  CircleX,
  Copy,
  Download,
  FolderGit2,
  FolderSync,
  GitBranch,
  GitGraph,
  GitMerge,
  Hand,
  KeyRound,
  LayoutDashboard,
  Link,
  LoaderCircle,
  Lock,
  LogOut,
  Pencil,
  Play,
  RotateCcw,
  RotateCw,
  Settings,
  Shield,
  Square,
  Trash2,
  TriangleAlert,
  Undo2,
  X,
} from "lucide-react";
import type { LucideIcon } from "lucide-react";

export type IconComponent = LucideIcon;

/** The one icon size: beside `text-xs` and `text-sm` labels alike. */
export const ICON_CLASS = "size-3.5 shrink-0";

export const Icon = {
  // Navigation.
  dashboard: LayoutDashboard,
  projects: FolderGit2,
  secrets: KeyRound,
  settings: Settings,
  admin: Shield,
  help: CircleQuestionMark,
  logout: LogOut,

  // Git.
  branch: GitBranch,
  fetch: Download,
  sync: FolderSync,
  merge: GitMerge,
  rebase: GitGraph,
  push: ArrowUpFromLine,
  revert: Undo2,
  /** Hand a conflicting merge to an agent session. */
  resolve: Bot,

  // Session controls.
  launch: Play,
  stop: Square,
  end: CircleStop,
  retry: RotateCcw,
  refresh: RotateCw,

  // Generic actions.
  edit: Pencil,
  delete: Trash2,
  close: X,
  copy: Copy,
  copied: Check,
  link: Link,
  expand: ChevronRight,
  collapse: ChevronDown,
  panelOpen: ChevronsLeft,
  panelClose: ChevronsRight,

  // States and notices.
  lock: Lock,
  warning: TriangleAlert,
  running: Activity,
  parked: CirclePause,
  failed: CircleX,
  done: CircleCheck,
  creating: LoaderCircle,
  needsHuman: Hand,
} as const satisfies Record<string, IconComponent>;
