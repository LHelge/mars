// Picking the commit a new session starts from (`SPEC.md`, "Projects":
// `GET /projects/{id}/branches`, and "Sessions": `base_ref` records the name
// the caller gave, not the commit it resolved to).
//
// The mirror's refs come back as one flat list of three different things, and
// choosing between them is the whole decision here, so they are grouped:
// integration heads are the everyday answer, upstream-tracking refs pick up
// what was last fetched, and a session ref continues another session's work.
// Tags and commit ids are not in the list at all, so the control keeps a
// `Custom ref` entry that opens a free-text field for them.
//
// An empty value means "do not send `base_ref`", which is how the server's own
// default — the task's hand-off commit when there is one, otherwise the
// project's default branch — is asked for.

import { useState } from "react";
import { FieldShell } from "../../components/FieldShell";
import { FIELD } from "../../components/fieldStyles";
import type { Branch } from "../../types";

/** The select entry that opens the free-text field. */
const CUSTOM = "\u0000custom";

export interface BaseRefSelectProps {
  branches: Branch[];
  /** The ref name to send, or `""` for the server's default. */
  value: string;
  onChange: (value: string) => void;
  /** What the empty entry is called: the hand-off commit, or the project default. */
  defaultLabel: string;
  disabled?: boolean;
  /** Shown under the control; the hand-off override notice goes here. */
  hint?: string;
}

const GROUPS: { kind: Branch["kind"]; label: string }[] = [
  { kind: "head", label: "Integration heads" },
  { kind: "upstream", label: "Upstream" },
  { kind: "session", label: "Session branches" },
];

export function BaseRefSelect({
  branches,
  value,
  onChange,
  defaultLabel,
  disabled,
  hint,
}: BaseRefSelectProps) {
  // Sticky once asked for: a ref the list does not know is custom by
  // definition, and clearing the field back to the default should not close
  // the field the user deliberately opened.
  const [customRequested, setCustomRequested] = useState(false);
  const known = branches.some((branch) => branch.name === value);
  const custom = customRequested || (value !== "" && !known);

  return (
    <FieldShell label="Base ref" name="session-base-ref" hint={hint}>
      {(control) => (
        <>
          <select
            {...control}
            value={custom ? CUSTOM : value}
            disabled={disabled}
            onChange={(event) => {
              const next = event.target.value;
              if (next === CUSTOM) {
                setCustomRequested(true);
                onChange("");
                return;
              }
              setCustomRequested(false);
              onChange(next);
            }}
            className={FIELD}
          >
            <option value="">{defaultLabel}</option>

            {GROUPS.map((group) => {
              const refs = branches.filter(
                (branch) => branch.kind === group.kind,
              );
              if (refs.length === 0) {
                return null;
              }
              return (
                <optgroup key={group.kind} label={group.label}>
                  {refs.map((branch) => (
                    <option key={branch.name} value={branch.name}>
                      {branch.name}
                    </option>
                  ))}
                </optgroup>
              );
            })}

            <option value={CUSTOM}>Custom ref…</option>
          </select>

          {custom && (
            <input
              id={`${control.id}-custom`}
              name="session-base-ref-custom"
              value={value}
              disabled={disabled}
              placeholder="A tag or commit id"
              aria-label="Custom base ref"
              aria-describedby={control["aria-describedby"]}
              onChange={(event) => {
                onChange(event.target.value);
              }}
              className={FIELD}
            />
          )}
        </>
      )}
    </FieldShell>
  );
}
