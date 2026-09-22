import { describe, expect, it } from "vitest";

import {
  HELP_TOPIC_TITLES,
  HELP_TOPICS,
  helpPath,
  parseHelpTopic,
} from "./topics";

describe("help topics", () => {
  it("parses every topic and nothing else", () => {
    for (const topic of HELP_TOPICS) {
      expect(parseHelpTopic(topic)).toBe(topic);
    }
    expect(parseHelpTopic("")).toBeUndefined();
    expect(parseHelpTopic("#secrets")).toBeUndefined();
    expect(parseHelpTopic("Secrets")).toBeUndefined();
    expect(parseHelpTopic("toString")).toBeUndefined();
  });

  it("points each topic at its anchor on /help", () => {
    expect(helpPath("git-credential")).toBe("/help#git-credential");
    expect(helpPath("getting-started")).toBe("/help#getting-started");
  });

  it("titles every topic", () => {
    for (const topic of HELP_TOPICS) {
      expect(HELP_TOPIC_TITLES[topic]).not.toBe("");
    }
  });
});
