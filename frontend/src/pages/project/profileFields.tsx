// The two shapes the profile editor's grants are made of: a bordered group
// with a legend and a line of explanation, and a list of checkboxes over the
// names a form field holds.
//
// Three grants are that list — git tools, served states, declared secrets —
// and each of them has a tail of names the project does not offer any more: a
// state that was renamed, a secret nobody has created. The tail is checkboxes
// over the same field, so it is the same list with different labels rather
// than a second rendering of one.

import type { ReactNode } from "react";

import { HelpLink } from "../../components/HelpLink";
import type { HelpTopic } from "../../help/topics";

import { CHECK_CLASS } from "./profileForm";

export interface FieldsetProps {
  legend: string;
  description: string;
  /** The help topic a `Learn more` link after the description opens. */
  help?: HelpTopic;
  children: ReactNode;
}

export function Fieldset({
  legend,
  description,
  help,
  children,
}: FieldsetProps) {
  return (
    <fieldset className="border-console-border rounded border p-3">
      <legend className="text-console-muted px-1 text-xs">{legend}</legend>
      <div className="text-console-muted pb-2 text-xs">
        <p className="inline">{description}</p>
        {help && (
          <>
            {" "}
            <HelpLink topic={help} />
          </>
        )}
      </div>
      {children}
    </fieldset>
  );
}

/** One checkbox of a grant list. */
export interface CheckboxItem<Name extends string = string> {
  /** The value the form's list holds, and the label's text. */
  name: Name;
  /** Said after the name, in the body font; the caller styles it. */
  note?: ReactNode;
  /**
   * Cannot be switched on. One that is already on can still be switched off,
   * so a grant nobody may add can still be given up.
   */
  locked?: boolean;
  /** The label's own classes, for a tail that reads differently. */
  className?: string;
}

const LABEL = "text-console-text flex items-center gap-2 font-mono text-xs";

export interface CheckboxListProps<Name extends string = string> {
  items: CheckboxItem<Name>[];
  /** The field's current value; an item is checked when it is in here. */
  selected: Name[];
  onToggle: (name: Name) => void;
  disabled: boolean;
}

/**
 * The labels alone, with no container of their own: the caller decides whether
 * they wrap in a row or stack in a column, and a tail rendered outside that
 * container is the same list with the same behaviour.
 */
export function CheckboxList<Name extends string>({
  items,
  selected,
  onToggle,
  disabled,
}: CheckboxListProps<Name>) {
  return (
    <>
      {items.map((item) => {
        const checked = selected.includes(item.name);
        return (
          <label key={item.name} className={item.className ?? LABEL}>
            <input
              type="checkbox"
              checked={checked}
              onChange={() => {
                onToggle(item.name);
              }}
              disabled={disabled || (item.locked === true && !checked)}
              className={CHECK_CLASS}
            />
            {item.name}
            {item.note}
          </label>
        );
      })}
    </>
  );
}
