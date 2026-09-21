// `SPEC.md`, "Projects", "Shared directories". Every project-level endpoint
// the projects and session views use; components never call `fetch`.

import type {
  Branch,
  Project,
  ProjectCreateInput,
  ProjectUpdateInput,
  SharedDir,
  SharedDirInput,
} from "../types";
import { apiDelete, apiGet, apiPost, apiPut, seg } from "./apiClient";

export function listProjects(): Promise<Project[]> {
  return apiGet<Project[]>("/projects");
}

export function getProject(id: string): Promise<Project> {
  return apiGet<Project>(`/projects/${seg(id)}`);
}

/** 201 with the created project, `status: cloning`. */
export function createProject(input: ProjectCreateInput): Promise<Project> {
  return apiPost<Project>("/projects", input);
}

export function updateProject(
  id: string,
  input: ProjectUpdateInput,
): Promise<Project> {
  return apiPut<Project>(`/projects/${seg(id)}`, input);
}

/** 204; refused with 409 while any session is `running` or `creating`. */
export function deleteProject(id: string): Promise<void> {
  return apiDelete(`/projects/${seg(id)}`);
}

/** Only from `status: error`. */
export function retryClone(id: string): Promise<Project> {
  return apiPost<Project>(`/projects/${seg(id)}/retry-clone`);
}

/** Runs a mirror fetch now, refreshing the upstream-tracking refs. */
export function fetchProject(id: string): Promise<Project> {
  return apiPost<Project>(`/projects/${seg(id)}/fetch`);
}

export function listBranches(id: string): Promise<Branch[]> {
  return apiGet<Branch[]>(`/projects/${seg(id)}/branches`);
}

export function listSharedDirs(id: string): Promise<SharedDir[]> {
  return apiGet<SharedDir[]>(`/projects/${seg(id)}/shared-dirs`);
}

export function createSharedDir(
  id: string,
  input: SharedDirInput,
): Promise<SharedDir> {
  return apiPost<SharedDir>(`/projects/${seg(id)}/shared-dirs`, input);
}

/** Empties the directory; 409 while any session of the project is live. */
export function clearSharedDir(id: string, name: string): Promise<void> {
  return apiPost<void>(
    `/projects/${seg(id)}/shared-dirs/${seg(name)}/clear`,
  );
}

/** Removes the directory and its contents; 409 while any session is live. */
export function deleteSharedDir(id: string, name: string): Promise<void> {
  return apiDelete(`/projects/${seg(id)}/shared-dirs/${seg(name)}`);
}
