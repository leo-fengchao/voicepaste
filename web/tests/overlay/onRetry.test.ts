import { act, renderHook } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

const { mockOnOverlayEvent, mockRetryPendingPolish, mockRetryLatestFailed } = vi.hoisted(() => ({
  mockOnOverlayEvent: vi.fn(
    (_listener: (event: { type: string; payload: Record<string, unknown> }) => void) => () => {},
  ),
  mockRetryPendingPolish: vi.fn(() => Promise.resolve(true)),
  mockRetryLatestFailed: vi.fn(() => Promise.resolve()),
}));

vi.mock("@/overlay/bridge", () => ({
  onOverlayEvent: mockOnOverlayEvent,
  retryPendingPolish: mockRetryPendingPolish,
  retryLatestFailedTranscription: mockRetryLatestFailed,
}));

import { useOverlayState } from "@/overlay/useOverlayState";

/** Deliver an overlay event through the listener the hook registered. */
function emitEvent(event: { type: string; payload: Record<string, unknown> }) {
  const listener = mockOnOverlayEvent.mock.calls[0]?.[0];
  act(() => listener?.(event));
}

/** Bring the hook into the retryable-error state the retry button requires. */
function armRetryableError() {
  emitEvent({
    type: "hint",
    payload: {
      text: "API Key 无效或已过期，请在设置中更新",
      level: "error",
      variant: "text",
      retryable: true,
      hotkey: "R ⌥",
      actions: "R ⌥ 重试 · Esc 放弃 · Enter 输出原文（默认）",
    },
  });
}

describe("overlay onRetry routing", () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it("retries a parked polish and skips the transcription retry when handled", async () => {
    mockRetryPendingPolish.mockResolvedValueOnce(true);
    const { result } = renderHook(() => useOverlayState());
    armRetryableError();

    await act(async () => {
      await result.current.onRetry();
    });

    expect(mockRetryPendingPolish).toHaveBeenCalledTimes(1);
    expect(mockRetryLatestFailed).not.toHaveBeenCalled();
  });

  it("falls back to the transcription retry when nothing is pending", async () => {
    mockRetryPendingPolish.mockResolvedValueOnce(false);
    const { result } = renderHook(() => useOverlayState());
    armRetryableError();

    await act(async () => {
      await result.current.onRetry();
    });

    expect(mockRetryPendingPolish).toHaveBeenCalledTimes(1);
    expect(mockRetryLatestFailed).toHaveBeenCalledTimes(1);
  });

  it("surfaces a command error as a retry-error hint", async () => {
    mockRetryPendingPolish.mockRejectedValueOnce("没有可重试的失败录音");
    const { result } = renderHook(() => useOverlayState());
    armRetryableError();

    await act(async () => {
      await result.current.onRetry();
    });

    expect(result.current.state.hintText).toBe("没有可重试的失败录音");
    expect(result.current.state.hintLevel).toBe("error");
    expect(result.current.state.retrying).toBe(false);
  });
});
