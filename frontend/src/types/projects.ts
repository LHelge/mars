// Mirrors `SPEC.md`, "Projects (`/api/projects`)"; `ProjectStatus` is the
// `project_status` enum of `docs/data-model.md`, "Enums".

export type ProjectStatus = "cloning" | "ready" | "error";

export interface Project {
  id: string;
  name: string;
  remote_url: string;
  default_branch: string | null;
  status: ProjectStatus;
  status_message: string | null;
  last_fetched_at: string | null;
  max_attempts: number;
  created_at: string;
  has_credential: boolean;
}
