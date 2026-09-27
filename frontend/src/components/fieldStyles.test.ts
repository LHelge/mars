// The pointer rule of `SPEC.md`, "Frontend", "Mobile layout", lives in these
// constants. A copy-paste that drops a `pointer-coarse:` class would leave
// the desktop unchanged and only a phone would notice, so the classes are
// asserted here.

import { describe, expect, it } from "vitest";

import {
  CHECK,
  CHECK_LABEL,
  CHECK_TOUCH,
  CONTROL,
  FIELD,
  TAP,
  TAP_INLINE,
  TOUCH_TEXT,
} from "./fieldStyles";

function classes(value: string): string[] {
  return value.split(/\s+/).filter((token) => token !== "");
}

describe("the pointer rule's constants", () => {
  it("gives every text control 16 px on a coarse pointer", () => {
    expect(TOUCH_TEXT).toBe("pointer-coarse:text-base");
    expect(classes(CONTROL)).toContain("pointer-coarse:text-base");
    expect(classes(FIELD)).toContain("pointer-coarse:text-base");
  });

  it("keeps the fine-pointer size of a control", () => {
    expect(classes(CONTROL)).toContain("text-sm");
  });

  it("gives TAP a 44 px box on a coarse pointer", () => {
    expect(classes(TAP)).toEqual([
      "pointer-coarse:min-h-11",
      "pointer-coarse:min-w-11",
    ]);
  });

  it("gives TAP_INLINE a 44 px pseudo-element on a coarse pointer only", () => {
    const tokens = classes(TAP_INLINE);
    expect(tokens).toContain("pointer-coarse:relative");
    expect(tokens).toContain("pointer-coarse:after:absolute");
    expect(tokens).toContain("pointer-coarse:after:min-h-11");
    expect(tokens).toContain("pointer-coarse:after:min-w-11");
    for (const token of tokens) {
      expect(token.startsWith("pointer-coarse:")).toBe(true);
    }
  });

  it("grows a checkbox to 20 px and its label to 44 px on a coarse pointer", () => {
    expect(CHECK_TOUCH).toBe("pointer-coarse:size-5");
    expect(classes(CHECK)).toContain("pointer-coarse:size-5");
    expect(classes(CHECK)).toContain("size-3.5");
    expect(CHECK_LABEL).toBe("pointer-coarse:min-h-11");
  });
});
