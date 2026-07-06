/** Prompt template item (mirrors Rust `PromptItem` in src-tauri/src/config.rs). */
export interface PromptItem {
  id: string;
  title: string;
  /** Accelerator string array, e.g. ["Control+Shift+A"]. Legacy numeric
   * keycodes are tolerated by the backend but the UI writes strings. */
  hotkey?: string[];
  hotkey_mode?: string;
  prompt?: string;
  /** Built-in classifier: true → auto-seeded for new users. Only present on
   * built-in templates read from the app resource; user data never carries it. */
  default_add?: boolean;
  /** UI-only cached hotkey display string (not persisted by the backend). */
  _displayString?: string;
}
