// Barrel for the project page's own parts, reached only from the lazy
// `ProjectPage` chunk (`ARCHITECTURE.md`, "Frontend architecture", Barrels).
// It carries what `ProjectPage` imports and nothing else: a name nobody imports
// through here is not re-exported, so the barrel cannot quietly grow into the
// hazard the directory barrels were.

export { ProjectHeader } from "./ProjectHeader";

export { ProjectSettingsForm } from "./ProjectSettingsForm";

export { BoardTab } from "./BoardTab";

export { ProfilesTab } from "./ProfilesTab";
export { SharedDirsTab } from "./SharedDirsTab";

export { ProjectTabs } from "./ProjectTabs";

export { SessionsTab } from "./SessionsTab";

export { LaunchSessionForm } from "./LaunchSessionForm";

export { PROJECT_TABS, parseProjectTab } from "./tabs";
export type { ProjectTab, ProjectTabPanelProps } from "./tabs";

export { useProject } from "./useProject";
