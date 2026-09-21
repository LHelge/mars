// The one launch both forms make (`SPEC.md`, "Sessions":
// `POST /projects/{pid}/sessions`; `SPEC.md`, "Frontend", "Task board" and
// "Hand-off controls").
//
// The project page's form and the task drawer's two buttons launch the same
// session, so create, the caches that go stale and the navigation are written
// once here and neither form keeps its own copy. What a form still decides is
// what it *sends*: which profile, whether there is a task, a message, a base.
//
// Two rules live in this hook because both forms need them and one of them
// had only half of each:
//
//   * Both caches. A launch creates a session, which is what the project's
//     Sessions tab lists, and — with a `task_id` — claims a task, which is
//     what the board and the open drawer show. Either read left alone is a
//     view that misses the launch until it polls: up to a minute for the
//     sessions list, until the next task event for the board.
//   * No navigation to a session the user is no longer waiting for. The form
//     can be gone before the answer arrives — the drawer closes on Escape, the
//     board navigates elsewhere — and pulling the user onto a session page
//     they have moved away from is the opposite of what they asked for. The
//     session was still created, so the caches are settled either way.
//
// The settle is deliberately not awaited, which is the one place a launch
// differs from every other write (`CLAUDE.md`, "Frontend conventions",
// "Submitting a form": the cache work is awaited inside the action). The
// reason a write awaits it is that the screen the user stays on must not claim
// to be current before the read that proves it. A launch leaves that screen:
// what proves the session exists is the session page this navigates to, and
// the refreshed drawer is unmounting. Awaiting here would hold the "one click"
// launch open for a full refetch of a panel nobody will see.

import { useCallback, useEffect, useRef } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { useNavigate } from "react-router";

import { queryKeys } from "../services/queryKeys";
import { createSession } from "../services/sessions";
import type { Session, SessionCreateInput } from "../types";
import { useRefetchTask, useSettleTask } from "../tasks/taskWrites";

/**
 * Launch a session in `projectId` and go to it.
 *
 * `taskNumber` is the task the launch claims, or `null` for a launch that
 * names none — the project page's form until a task is typed into it. It is
 * what the drawer's copy of the task is reread by, on the way out and after a
 * refusal alike.
 */
export function useLaunchSession(
  projectId: string,
  taskNumber: number | null,
): (input: SessionCreateInput) => Promise<Session> {
  const navigate = useNavigate();
  const queryClient = useQueryClient();
  const settle = useSettleTask(projectId, taskNumber);
  const refetchTask = useRefetchTask(projectId, taskNumber);

  // Whether the form that started this launch is still on screen. A ref, not
  // state: nothing renders from it, and it is read after an await.
  const live = useRef(true);
  useEffect(() => {
    live.current = true;
    return () => {
      live.current = false;
    };
  }, []);

  return useCallback(
    async (input: SessionCreateInput): Promise<Session> => {
      let session: Session;
      try {
        session = await createSession(projectId, input);
      } catch (caught) {
        // A 409 is somebody else holding the task now, or a hand-off that
        // moved; any other refusal leaves the same doubt about what the drawer
        // is showing. Either way the task is what to read again — the rule
        // every task write follows (`tasks/taskWrites.ts`).
        await refetchTask();
        throw caught;
      }

      // Fire-and-forget, for the reason at the top of this file.
      void settle();
      void queryClient.invalidateQueries({
        queryKey: queryKeys.projects.sessions(projectId),
      });

      if (live.current) {
        void navigate(`/sessions/${session.id}`);
      }
      return session;
    },
    [projectId, navigate, queryClient, settle, refetchTask],
  );
}
