// Barrel for the project page's own parts. Later tasks of this epic register
// their tab panels through `ProjectTabs`.

export { ProjectHeader } from "./ProjectHeader";
export type { ProjectHeaderProps } from "./ProjectHeader";

export { ProjectSettingsForm } from "./ProjectSettingsForm";
export type { ProjectSettingsFormProps } from "./ProjectSettingsForm";

export { ProjectTabs } from "./ProjectTabs";
export type { ProjectTabsProps } from "./ProjectTabs";

export { DEFAULT_PROJECT_TAB, PROJECT_TABS, parseProjectTab } from "./tabs";
export type { ProjectTab, ProjectTabPanelProps } from "./tabs";

export { isNotFound, isUuid, projectErrorMessage } from "./messages";

export { useProject } from "./useProject";
