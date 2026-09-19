// `SPEC.md`, "Projects". The dashboard needs only the list, to turn the
// `project_id` on a session or task into a name; the projects epic extends
// this module.

import type { Project } from "../types";
import { apiGet } from "./apiClient";

export function listProjects(): Promise<Project[]> {
  return apiGet<Project[]>("/projects");
}
