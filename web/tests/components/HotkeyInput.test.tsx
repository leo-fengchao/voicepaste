import "@testing-library/jest-dom/vitest";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

// Mock the bridge so clicking the field drives a fake recorder instead of IPC.
const { mockRecordHotkey } = vi.hoisted(() => ({
  mockRecordHotkey:
    vi.fn<() => Promise<{ keys: string[]; displayString: string; hotkey?: string }>>(),
}));

vi.mock("@/settings/bridge", () => ({
  recordHotkey: mockRecordHotkey,
}));

import { HotkeyInput } from "@/settings/components/HotkeyInput";

afterEach(() => {
  cleanup();
  vi.clearAllMocks();
});

describe("HotkeyInput", () => {
  it("renders the bound hotkey as per-platform tokens", () => {
    render(<HotkeyInput value="ControlLeft+ShiftLeft" isMac={false} />);
    const field = screen.getByLabelText("快捷键");
    expect(field).toHaveTextContent("Ctrl");
    expect(field).toHaveTextContent("Shift");
  });

  it("shows the capture placeholder when unbound", () => {
    render(<HotkeyInput value="" isMac={false} />);
    expect(screen.getByText("按下快捷键")).toBeInTheDocument();
  });

  it("starts recording on click and commits the captured combination", async () => {
    mockRecordHotkey.mockResolvedValue({ keys: ["F13"], displayString: "F13", hotkey: "F13" });
    const onCommit = vi.fn();
    const onStart = vi.fn();
    const onDone = vi.fn();
    render(
      <HotkeyInput value="" isMac={false} onCommit={onCommit} onStart={onStart} onDone={onDone} />,
    );

    fireEvent.click(screen.getByLabelText("快捷键"));

    await waitFor(() => expect(mockRecordHotkey).toHaveBeenCalledTimes(1));
    expect(onStart).toHaveBeenCalledTimes(1);
    expect(onDone).toHaveBeenCalledTimes(1);
    expect(onCommit).toHaveBeenCalledWith("F13", "F13");
  });

  it("does not commit when recording is cancelled (empty keys)", async () => {
    mockRecordHotkey.mockResolvedValue({ keys: [], displayString: "", hotkey: undefined });
    const onCommit = vi.fn();
    const onDone = vi.fn();
    render(<HotkeyInput value="" isMac={false} onCommit={onCommit} onDone={onDone} />);

    fireEvent.click(screen.getByLabelText("快捷键"));

    await waitFor(() => expect(mockRecordHotkey).toHaveBeenCalled());
    expect(onDone).toHaveBeenCalledTimes(1);
    expect(onCommit).not.toHaveBeenCalled();
  });

  it("clears the hotkey via the × button", () => {
    const onClear = vi.fn();
    render(<HotkeyInput value="F13" isMac={false} onClear={onClear} />);
    fireEvent.click(screen.getByLabelText("清除快捷键"));
    expect(onClear).toHaveBeenCalledTimes(1);
  });

  it("hides the × button when there is no value", () => {
    render(<HotkeyInput value="" isMac={false} onClear={vi.fn()} />);
    expect(screen.queryByLabelText("清除快捷键")).not.toBeInTheDocument();
  });

  it("does not record when locked (display-only reference field)", () => {
    render(<HotkeyInput value="F13" isMac={false} locked />);
    // Locked fields render as a disabled button (not focusable) with no clear button.
    expect(screen.getByLabelText("快捷键")).toBeDisabled();
    expect(screen.queryByLabelText("清除快捷键")).not.toBeInTheDocument();
    expect(mockRecordHotkey).not.toHaveBeenCalled();
  });

  it("does not start recording while another field is busy", () => {
    mockRecordHotkey.mockResolvedValue({ keys: ["F13"], displayString: "F13", hotkey: "F13" });
    render(<HotkeyInput value="" isMac={false} busy />);
    fireEvent.click(screen.getByLabelText("快捷键"));
    expect(mockRecordHotkey).not.toHaveBeenCalled();
  });
});
