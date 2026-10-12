export type AppState = "idle" | "connecting" | "recording" | "finishing";
export type HintLevel = "info" | "error" | "warn";

/** Logical overlay state mirrored from backend `overlay:event`s. Audio level is
 * kept out of here (high-frequency; lives in a ref to avoid re-renders) and
 * layout width/wrap are owned by useOverlayLayout. */
export interface OverlayState {
  finalText: string;
  partialText: string;
  hintText: string;
  hintLevel: HintLevel;
  hintVariant: string;
  /** Actions line under the hint (e.g. "R ⌥ 重试 · Esc 丢弃 · Enter 输出原文"),
   * naming the keys that act on the current state. Empty when not applicable. */
  hintActions: string;
  retryHotkey: string;
  appState: AppState;
  retryVisible: boolean;
  retrying: boolean;
}

export const INITIAL_OVERLAY_STATE: OverlayState = {
  finalText: "",
  partialText: "",
  hintText: "",
  hintLevel: "info",
  hintVariant: "text",
  hintActions: "",
  retryHotkey: "",
  appState: "idle",
  retryVisible: false,
  retrying: false,
};
