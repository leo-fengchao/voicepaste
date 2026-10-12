import { describe, expect, it } from "vitest";
import { INITIAL_OVERLAY_STATE } from "@/overlay/types";
import { reducer } from "@/overlay/useOverlayState";

describe("overlay reducer", () => {
  describe("hint with actions", () => {
    it("stores the actions line alongside the hint", () => {
      const next = reducer(INITIAL_OVERLAY_STATE, {
        type: "hint",
        text: "润色未配置完整：缺少 llm.deepseek.api_key",
        level: "error",
        variant: "text",
        retryable: true,
        hotkey: "R ⌥",
        actions: "R ⌥ 重试 · Esc 放弃 · Enter 输出原文（默认）",
      });
      expect(next.hintActions).toBe("R ⌥ 重试 · Esc 放弃 · Enter 输出原文（默认）");
      expect(next.retryVisible).toBe(true);
    });

    it("clears the actions line when the hint carries none", () => {
      const armed = reducer(INITIAL_OVERLAY_STATE, {
        type: "hint",
        text: "模型响应超时",
        level: "error",
        actions: "Esc 放弃 · Enter 输出原文",
      });
      const next = reducer(armed, { type: "hint", text: "准备中…", level: "info" });
      expect(next.hintActions).toBe("");
    });

    it("entering a state clears an info-level hint together with its actions", () => {
      const polishing = reducer(INITIAL_OVERLAY_STATE, {
        type: "hint",
        text: "润色中…",
        level: "info",
        variant: "progress",
        actions: "Esc 放弃 · Enter 输出原文",
      });
      const next = reducer(polishing, { type: "state", state: "idle" });
      expect(next.hintText).toBe("");
      expect(next.hintActions).toBe("");
    });

    it("a retry error replaces the hint and drops the actions line", () => {
      const armed = reducer(INITIAL_OVERLAY_STATE, {
        type: "hint",
        text: "API Key 无效或已过期，请在设置中更新",
        level: "error",
        actions: "Esc 放弃 · Enter 输出原文",
      });
      const next = reducer(armed, { type: "retry-error", message: "重试失败" });
      expect(next.hintText).toBe("重试失败");
      expect(next.hintActions).toBe("");
      expect(next.retryVisible).toBe(true);
    });
  });
});
