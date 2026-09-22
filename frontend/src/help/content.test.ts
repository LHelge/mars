import { describe, expect, it } from "vitest";

import {
  HELP_SOURCES,
  helpContent,
  helpLinkTargets,
  resolveHelpLinks,
} from "./content";
import { HELP_TOPICS, parseHelpTopic } from "./topics";

describe("help content", () => {
  it("has prose for every topic", () => {
    for (const topic of HELP_TOPICS) {
      expect(HELP_SOURCES[topic].trim()).not.toBe("");
    }
  });

  it("links only to topics that exist", () => {
    for (const topic of HELP_TOPICS) {
      for (const target of helpLinkTargets(HELP_SOURCES[topic])) {
        expect(parseHelpTopic(target), `${topic} links help:${target}`).toBe(
          target,
        );
      }
    }
  });

  it("resolves a help: link to the topic's path and leaves an unknown one", () => {
    expect(resolveHelpLinks("See [secrets](help:secrets).")).toBe(
      "See [secrets](/help#secrets).",
    );
    expect(resolveHelpLinks("[x](help:nowhere)")).toBe("[x](help:nowhere)");
    expect(resolveHelpLinks("[x](https://example.invalid)")).toBe(
      "[x](https://example.invalid)",
    );
  });

  it("serves no help: link unresolved", () => {
    for (const topic of HELP_TOPICS) {
      expect(helpContent(topic)).not.toContain("](help:");
    }
  });
});
