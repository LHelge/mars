// Mirrors the error envelope of `SPEC.md`, "REST API".
// Timestamps are RFC 3339 strings and ids are UUID strings throughout.

export interface ApiErrorBody {
  status: number;
  error: string;
  /** Present only on 422 git-conflict responses. */
  conflicts?: string[];
}
