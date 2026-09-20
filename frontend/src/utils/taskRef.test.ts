import { describe, expect, it } from "vitest";
import { parseTaskRef } from "./taskRef";

// An obviously fake id, in the shape the API spells its ids.
const UUID = "11111111-2222-4333-8444-555555555555";

describe("parseTaskRef", () => {
  it("reads a hash-prefixed number", () => {
    expect(parseTaskRef("#12")).toEqual({ kind: "number", number: 12 });
  });

  it("reads a bare number", () => {
    expect(parseTaskRef("12")).toEqual({ kind: "number", number: 12 });
  });

  it("reads a UUID", () => {
    expect(parseTaskRef(UUID)).toEqual({ kind: "uuid", id: UUID });
  });

  it("lower-cases a UUID typed in capitals", () => {
    expect(parseTaskRef(UUID.toUpperCase())).toEqual({
      kind: "uuid",
      id: UUID,
    });
  });

  it("ignores surrounding whitespace", () => {
    expect(parseTaskRef("  #7 ")).toEqual({ kind: "number", number: 7 });
  });

  it("refuses an empty field", () => {
    expect(parseTaskRef("")).toBeNull();
    expect(parseTaskRef("   ")).toBeNull();
  });

  it("refuses zero, because task numbers start at one", () => {
    expect(parseTaskRef("#0")).toBeNull();
  });

  it("refuses anything that is neither a number nor a UUID", () => {
    expect(parseTaskRef("fix the parser")).toBeNull();
    expect(parseTaskRef("-3")).toBeNull();
    expect(parseTaskRef("12a")).toBeNull();
    expect(parseTaskRef(UUID.slice(0, 20))).toBeNull();
  });
});
