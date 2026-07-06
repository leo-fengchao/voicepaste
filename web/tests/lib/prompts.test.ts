import { describe, expect, it } from "vitest";
import { cloneBuiltinPrompt } from "@/settings/lib/prompts";
import type { PromptItem } from "@/settings/types/prompts";

describe("cloneBuiltinPrompt", () => {
  const source: PromptItem = {
    id: "prompt-general",
    title: "通用整理",
    hotkey: ["ControlLeft+ShiftLeft"],
    hotkey_mode: "toggle",
    prompt: "整理语音转写内容",
    default_add: true,
  };

  it("assigns a new id different from the source", () => {
    const clone = cloneBuiltinPrompt(source);
    expect(clone.id).not.toBe(source.id);
    expect(clone.id).toBeTruthy();
  });

  it("copies title, hotkey_mode and prompt verbatim", () => {
    const clone = cloneBuiltinPrompt(source);
    expect(clone.title).toBe(source.title);
    expect(clone.hotkey_mode).toBe(source.hotkey_mode);
    expect(clone.prompt).toBe(source.prompt);
  });

  it("copies hotkey into an independent array", () => {
    const clone = cloneBuiltinPrompt(source);
    expect(clone.hotkey).toEqual(source.hotkey);
    clone.hotkey?.push("X");
    expect(source.hotkey).toEqual(["ControlLeft+ShiftLeft"]);
  });

  it("drops the built-in default_add flag", () => {
    const clone = cloneBuiltinPrompt(source);
    expect(clone.default_add).toBeUndefined();
  });
});
