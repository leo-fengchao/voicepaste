/**
 * Prompt template helpers.
 */
import type { PromptItem } from "@/settings/types/prompts";

/** Clone a built-in prompt template into a fresh custom template.
 *
 * Generates a new unique id so the clone is independent of its built-in
 * source — editing or deleting it never collides with or gets re-merged from
 * the built-in library on a future app update. `default_add` and
 * `_displayString` are dropped so the user copy stays pure. */
export function cloneBuiltinPrompt(item: PromptItem): PromptItem {
  return {
    id: crypto.randomUUID(),
    title: item.title,
    hotkey: item.hotkey,
    hotkey_mode: item.hotkey_mode,
    prompt: item.prompt,
  };
}
