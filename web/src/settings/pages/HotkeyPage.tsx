import { useCallback, useEffect, useState } from "react";
import { loadPrompts, savePrompts } from "@/settings/bridge";
import { HotkeyInput } from "@/settings/components/HotkeyInput";
import { KeyCap } from "@/settings/components/KeyCap";
import { SegmentedControl } from "@/settings/components/SegmentedControl";
import { Toast, useToast } from "@/settings/components/Toast";
import {
  PageHeader,
  PageLayout,
  Section,
  SectionContent,
  SectionHeader,
  SectionItem,
  SectionItemList,
} from "@/settings/layout/PageLayout";
import { normalizeHotkeyToken } from "@/settings/lib/hotkey";
import { useSettings } from "@/settings/SettingsProvider";
import type { PromptItem } from "@/settings/types/prompts";

const MODE_OPTIONS = [
  { value: "toggle", label: "免提模式" },
  { value: "hold", label: "按住说话" },
];

export function HotkeyPage() {
  const { settings, scheduleSave } = useSettings();
  const cfg = settings?.parsedConfig || ({} as Record<string, unknown>);
  const app = (cfg.app || {}) as Record<string, string | undefined>;
  // Keep an explicit empty string (user cleared the key) distinct from missing.
  const hotkeyStr = app.hotkey ?? "F13";
  const hotkeyMode = (app.hotkey_mode as string) || "toggle";
  // Show Apple symbols on macOS, native labels (Ctrl/Alt/Win/Shift) on Windows.
  const isMac = settings?.runtime?.platform === "macos";
  const { toast, showToast } = useToast();

  const [prompts, setPrompts] = useState<PromptItem[]>([]);
  // Which field is currently capturing — only one recorder runs at a time, so
  // every other recordable field goes `busy` while it is set.
  const [activeRecorder, setActiveRecorder] = useState<string | null>(null);

  useEffect(() => {
    loadPrompts()
      .then((d) => {
        if (Array.isArray(d)) setPrompts(d as unknown as PromptItem[]);
      })
      .catch(() => {});
  }, []);

  const isBusyFor = useCallback(
    (id: string) => activeRecorder !== null && activeRecorder !== id,
    [activeRecorder],
  );

  // Immutable update of a single prompt + persist. Shared by mode switches and
  // record/clear of the prompt's own hotkey.
  const updatePrompt = useCallback(
    async (index: number, patch: Partial<PromptItem>) => {
      const updated = prompts.map((p, i) => (i === index ? { ...p, ...patch } : p));
      setPrompts(updated);
      await savePrompts(updated as unknown[]);
    },
    [prompts],
  );

  // The recordable hotkey field for one prompt row — its "stop" key in toggle
  // mode, its single key in hold mode. The "start" side is rendered separately
  // as the locked global main key.
  const promptRecorderProps = useCallback(
    (item: PromptItem, index: number) => {
      const recorderId = `prompt:${item.id}`;
      return {
        isMac,
        busy: isBusyFor(recorderId),
        onStart: () => setActiveRecorder(recorderId),
        onDone: () => setActiveRecorder(null),
        onCommit: (hk: string) => updatePrompt(index, { hotkey: hk }),
        onClear: () => updatePrompt(index, { hotkey: "" }),
      };
    },
    [isMac, isBusyFor, updatePrompt],
  );

  return (
    <PageLayout>
      <PageHeader title="快捷键" description="为语音输入配置自定义快捷键。">
        <div className="flex flex-col gap-1 mt-2 text-text-muted">
          <p>免提模式：按一次「语音唤起」开始，再按一次结束，或按润色快捷键在结束时润色；</p>
          <p>按住说话：按住对应快捷键开始，松开结束。</p>
          <p>
            录制快捷键时按 <KeyCap label={normalizeHotkeyToken("ESC", isMac).main} /> 取消
          </p>
        </div>
      </PageHeader>

      {/* Section 1 — global main key (the hands-free wake key) */}
      <Section>
        <SectionHeader
          title="全局语音主键"
          subtitle="免提模式的唤起键 / 按住说话键；所有免提模式均以此键开始录音"
        />
        <SectionContent>
          <SectionItemList>
            <SectionItem
              title="语音唤起"
              last
              action={
                <HotkeyInput
                  value={hotkeyStr}
                  isMac={isMac}
                  busy={isBusyFor("main")}
                  onStart={() => setActiveRecorder("main")}
                  onDone={() => setActiveRecorder(null)}
                  onCommit={(hk) => scheduleSave({ app: { hotkey: hk } })}
                  onClear={() => scheduleSave({ app: { hotkey: "" } })}
                  validateFirstKey
                  onInvalid={(reason) => showToast(reason, "error")}
                />
              }
            />
          </SectionItemList>
        </SectionContent>
      </Section>

      {/* Section 2 — per-feature hotkeys (voice input + each polish prompt) */}
      <Section>
        <SectionHeader title="快捷键" subtitle="点击输入框进入录制 → 按下快捷键" />
        <SectionContent>
          <SectionItemList>
            <SectionItem
              title="语音输入"
              action={
                <div className="flex items-center gap-2">
                  {hotkeyMode === "toggle" ? (
                    <>
                      {/* start = main key (locked); stop = press main key again (locked) */}
                      <HotkeyInput value={hotkeyStr} isMac={isMac} locked />
                      <span className="text-xs text-text-muted">→</span>
                      <HotkeyInput value={hotkeyStr} isMac={isMac} locked />
                    </>
                  ) : (
                    <HotkeyInput value={hotkeyStr} isMac={isMac} locked />
                  )}
                  <SegmentedControl
                    options={MODE_OPTIONS}
                    value={hotkeyMode}
                    onChange={(v) => scheduleSave({ app: { hotkey_mode: v } })}
                  />
                </div>
              }
            />

            {prompts.map((item, idx) => {
              const mode = item.hotkey_mode || "toggle";
              return (
                <SectionItem
                  key={item.id}
                  title={item.title}
                  last={idx === prompts.length - 1}
                  action={
                    <div className="flex items-center gap-2">
                      {mode === "toggle" ? (
                        <>
                          {/* start = main key (locked); stop = this prompt's polish key */}
                          <HotkeyInput value={hotkeyStr} isMac={isMac} locked />
                          <span className="text-xs text-text-muted">→</span>
                          <HotkeyInput
                            value={item.hotkey || ""}
                            {...promptRecorderProps(item, idx)}
                          />
                        </>
                      ) : (
                        <HotkeyInput
                          value={item.hotkey || ""}
                          {...promptRecorderProps(item, idx)}
                        />
                      )}
                      <SegmentedControl
                        options={MODE_OPTIONS}
                        value={mode}
                        onChange={(v) => updatePrompt(idx, { hotkey_mode: v })}
                      />
                    </div>
                  }
                />
              );
            })}
          </SectionItemList>
        </SectionContent>
      </Section>
      <Toast toast={toast} />
    </PageLayout>
  );
}
