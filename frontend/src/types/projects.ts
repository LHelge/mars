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
  /**
   * How many live sessions the project may have before an unattended launch
   * is held back; `null` is no cap. Binds automation only — a launch by a
   * person is never refused by it (`ARCHITECTURE.md`, "Task tracker" →
   * "Unattended launches").
   */
  max_concurrent_sessions: number | null;
  /** While set, no unattended launch happens in this project. */
  automation_paused: boolean;
  created_at: string;
  has_credential: boolean;
}

/**
 * `POST /projects`. `credential` is stored as the project-scoped
 * orchestrator-only `GIT_CREDENTIAL` secret and never comes back.
 */
export interface ProjectCreateInput {
  name: string;
  remote_url: string;
  default_branch?: string;
  credential?: string;
}

/** `PUT /projects/{id}`; every field is optional and only the given ones change. */
export interface ProjectUpdateInput {
  name?: string;
  default_branch?: string;
  /** 1–20; how many claims in one state escalate a task. */
  max_attempts?: number;
  /**
   * At least 1 when set. An explicit `null` removes the cap; omitting the key
   * leaves the stored one alone (`SPEC.md`, "Projects").
   */
  max_concurrent_sessions?: number | null;
  automation_paused?: boolean;
}

/**
 * `GET /projects/{id}/branches`: integration heads (`main`), upstream-tracking
 * refs (`origin/main`) and session refs (`refs/sessions/<id>`, with `session_id`).
 */
export type BranchKind = "head" | "upstream" | "session";

export interface Branch {
  name: string;
  kind: BranchKind;
  commit: string;
  session_id?: string;
}

/**
 * Mirrors `SPEC.md`, "Shared directories": a directory bind-mounted read-write
 * at `container_path` in every session container of the project.
 */
export interface SharedDir {
  name: string;
  container_path: string;
  created_at: string;
}

export interface SharedDirInput {
  name: string;
  container_path: string;
}
