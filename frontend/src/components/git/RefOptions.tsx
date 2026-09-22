// One kind of ref as a select's optgroup: upstream, integration heads or
// session branches (`SPEC.md`, "Git": `GET /projects/{pid}/git/branches`
// answers each ref with its `kind`).
//
// A kind the mirror has none of renders nothing at all, so a project with no
// session branches does not offer an empty group to open.

import type { Branch } from "../../types";
import { refsOfKind } from "./formState";

export interface RefOptionsProps {
  branches: Branch[];
  kind: Branch["kind"];
  label: string;
}

export function RefOptions({ branches, kind, label }: RefOptionsProps) {
  const refs = refsOfKind(branches, kind);
  if (refs.length === 0) {
    return null;
  }
  return (
    <optgroup label={label}>
      {refs.map((branch) => (
        <option key={branch.name} value={branch.name}>
          {branch.name}
        </option>
      ))}
    </optgroup>
  );
}
