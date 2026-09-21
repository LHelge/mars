// Which agent profile a launch runs (`SPEC.md`, "User-facing features", Agent
// profiles). Both launch forms offer the same control over the same list; what
// they differ in is which profiles they offer and how an entry reads, so those
// are the props.
//
// The selection itself is derived, never stored: the caller passes the profile
// it has decided on — the one the user picked, otherwise the project's default
// or the first that serves the task's state — so the right entry is selected
// the moment the list arrives, without an effect writing state behind the
// render.

import { FieldShell } from "../components/FieldShell";
import { FIELD } from "../components/fieldStyles";
import type { Profile } from "../types";

export interface ProfileSelectProps {
  /** The field's name and the stem of its id; the two forms name it apart. */
  name: string;
  /** The profiles to offer, already filtered to the ones this form may launch. */
  profiles: Profile[];
  /** The profile currently chosen, or `undefined` when there is none to choose. */
  selected: Profile | undefined;
  onChange: (profileId: string) => void;
  /** How one entry reads: its role, or the states it serves. */
  optionLabel: (profile: Profile) => string;
  /** The single entry shown when there is nothing to offer, if the form wants one. */
  emptyLabel?: string;
  disabled?: boolean;
}

export function ProfileSelect({
  name,
  profiles,
  selected,
  onChange,
  optionLabel,
  emptyLabel,
  disabled,
}: ProfileSelectProps) {
  return (
    <FieldShell label="Agent profile" name={name}>
      {(control) => (
        <select
          {...control}
          value={selected?.id ?? ""}
          disabled={disabled === true || profiles.length === 0}
          onChange={(event) => {
            onChange(event.target.value);
          }}
          className={FIELD}
        >
          {profiles.length === 0 && emptyLabel !== undefined && (
            <option value="">{emptyLabel}</option>
          )}
          {profiles.map((profile) => (
            <option key={profile.id} value={profile.id}>
              {optionLabel(profile)}
            </option>
          ))}
        </select>
      )}
    </FieldShell>
  );
}
