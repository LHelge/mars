// `/secrets` (`SPEC.md`, "Frontend", Routes): the scope selector over the
// reusable `SecretsManager`. The selection lives in the URL search params
// `?scope=&scope_id=` so a reload keeps it and a link carries it.
//
// Four choices, three scopes: `global`, a project, the caller's own user
// scope, and — for an administrator — another user's. `scope=user` with the
// caller's own id is the same thing as "my secrets", so the URL is normalised
// to drop the id (`SPEC.md`, "Secrets": a user scope with no `scope_id` is the
// caller's own).

import { useQuery } from "@tanstack/react-query";
import { useEffect } from "react";
import { useSearchParams } from "react-router";
import { Alert, PageLayout } from "../components";
import { SecretsManager } from "../components/secrets/SecretsManager";
import { useAuth } from "../hooks/useAuth";
// By path, not through a barrel: the section is only ever on this lazily
// loaded route, and the barrel is in the entry chunk (`SPEC.md`, "Frontend" →
// "Code splitting"), as with `DiffBody`.
import { AgentCredentialsSection } from "../secrets/AgentCredentialsSection";
import { listProjects } from "../services/projects";
import { queryKeys } from "../services/queryKeys";
import { listUsers } from "../services/users";
import type { SecretScope } from "../types";

/** What the radio group offers; three of the four map onto one API scope. */
type Choice = "global" | "project" | "mine" | "user";

const CHOICES: { value: Choice; label: string; adminOnly?: boolean }[] = [
  { value: "global", label: "Global" },
  { value: "project", label: "Project" },
  { value: "mine", label: "My secrets" },
  { value: "user", label: "Another user", adminOnly: true },
];

const SELECT_CLASS =
  "border-console-border bg-console-bg text-console-text rounded border px-2 py-1 font-mono text-xs";

export function SecretsPage() {
  const { user, isAdmin } = useAuth();
  const [params, setParams] = useSearchParams();

  const rawScope = params.get("scope");
  const scope: SecretScope =
    rawScope === "project" || rawScope === "user" ? rawScope : "global";
  const rawScopeId = params.get("scope_id");
  const scopeId = rawScopeId === null || rawScopeId === "" ? null : rawScopeId;

  // A user scope naming the caller is "my secrets": normalise it away so both
  // spellings share one URL and one query key.
  const selfSelected = scope === "user" && user !== null && scopeId === user.id;

  useEffect(() => {
    if (!selfSelected) {
      return;
    }
    const next = new URLSearchParams(params);
    next.delete("scope_id");
    setParams(next, { replace: true });
  }, [selfSelected, params, setParams]);

  const choice: Choice =
    scope === "project"
      ? "project"
      : scope === "user" && scopeId !== null && !selfSelected
        ? "user"
        : scope === "user"
          ? "mine"
          : "global";

  const projects = useQuery({
    queryKey: queryKeys.projects.list(),
    queryFn: listProjects,
  });

  const users = useQuery({
    queryKey: queryKeys.users.list(),
    queryFn: listUsers,
    enabled: isAdmin,
  });

  function select(next: Choice) {
    const search = new URLSearchParams();
    if (next === "global") {
      search.set("scope", "global");
    } else if (next === "mine") {
      search.set("scope", "user");
    } else if (next === "project") {
      search.set("scope", "project");
      const first = projects.data?.[0]?.id;
      if (first !== undefined) {
        search.set("scope_id", first);
      }
    } else {
      search.set("scope", "user");
      const first = users.data?.find((candidate) => candidate.id !== user?.id);
      if (first !== undefined) {
        search.set("scope_id", first.id);
      }
    }
    setParams(search);
  }

  function selectId(id: string) {
    const search = new URLSearchParams();
    search.set("scope", scope);
    search.set("scope_id", id);
    setParams(search);
  }

  const projectName = projects.data?.find((p) => p.id === scopeId)?.name;
  const userName = users.data?.find((u) => u.id === scopeId)?.username;

  const title =
    choice === "global"
      ? "Global secrets"
      : choice === "mine"
        ? "My secrets"
        : choice === "project"
          ? `Project secrets${projectName === undefined ? "" : `: ${projectName}`}`
          : `User secrets${userName === undefined ? "" : `: ${userName}`}`;

  // A project or another user has to be picked before there is a scope to read.
  const needsId =
    (choice === "project" || choice === "user") && scopeId === null;

  // An administrator looking at somebody else's scope: their credentials join
  // the section above under their username rather than `You`.
  const otherUser =
    choice === "user" && scopeId !== null && userName !== undefined
      ? { id: scopeId, username: userName }
      : undefined;

  return (
    <PageLayout title="Secrets">
      <div className="space-y-8">
        <AgentCredentialsSection
          {...(otherUser === undefined ? {} : { otherUser })}
        />

        <div className="space-y-5">
          <fieldset className="flex flex-wrap items-center gap-x-4 gap-y-2">
            <legend className="text-console-muted mb-1 text-xs">Scope</legend>

            {CHOICES.filter((entry) => !entry.adminOnly || isAdmin).map(
              (entry) => (
                <label
                  key={entry.value}
                  className="text-console-text flex items-center gap-1.5 text-sm"
                >
                  <input
                    type="radio"
                    name="secret-scope"
                    value={entry.value}
                    checked={choice === entry.value}
                    onChange={() => {
                      select(entry.value);
                    }}
                    className="accent-console-accent size-3.5"
                  />
                  {entry.label}
                </label>
              ),
            )}

            {choice === "project" && (
              <select
                aria-label="Project"
                value={scopeId ?? ""}
                onChange={(event) => {
                  selectId(event.target.value);
                }}
                className={SELECT_CLASS}
              >
                <option value="" disabled>
                  Choose a project
                </option>
                {(projects.data ?? []).map((project) => (
                  <option key={project.id} value={project.id}>
                    {project.name}
                  </option>
                ))}
              </select>
            )}

            {choice === "user" && isAdmin && (
              <select
                aria-label="User"
                value={scopeId ?? ""}
                onChange={(event) => {
                  selectId(event.target.value);
                }}
                className={SELECT_CLASS}
              >
                <option value="" disabled>
                  Choose a user
                </option>
                {(users.data ?? []).map((candidate) => (
                  <option key={candidate.id} value={candidate.id}>
                    {candidate.username}
                  </option>
                ))}
              </select>
            )}
          </fieldset>

          {needsId ? (
            <Alert kind="info">
              {choice === "project"
                ? "Choose a project to see its secrets."
                : "Choose a user to see their secrets."}
            </Alert>
          ) : (
            <SecretsManager
              scope={scope}
              {...(scopeId === null || selfSelected ? {} : { scopeId })}
              title={title}
              hideAgentCredentials
            />
          )}
        </div>
      </div>
    </PageLayout>
  );
}
