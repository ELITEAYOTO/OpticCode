import type { ChatUiTiming } from '../protocol/types';

export function createChatUiTiming(
  requestId: string,
  handlerStartedAtMs: number,
  firstContentAtMs: number | undefined,
  lastContentAtMs: number | undefined,
  terminalRenderStartedAtMs: number,
): ChatUiTiming {
  const firstTokenMs = elapsedBetween(handlerStartedAtMs, firstContentAtMs);
  const visibleResponseMs = elapsedBetween(handlerStartedAtMs, lastContentAtMs);
  const totalPipelineMs = elapsedBetween(
    handlerStartedAtMs,
    terminalRenderStartedAtMs,
  ) ?? 0;
  const timing: ChatUiTiming = {
    schema_version: 1,
    request_id: requestId,
    clock: 'performance.now',
    first_token_ms: firstTokenMs,
    answer_streaming_ms:
      elapsedBetween(firstContentAtMs, lastContentAtMs) ?? 0,
    visible_response_ms: visibleResponseMs,
    total_pipeline_ms: totalPipelineMs,
    post_processing_ms:
      visibleResponseMs === null ? 0 : Math.max(0, totalPipelineMs - visibleResponseMs),
    terminal_rendered_ms: totalPipelineMs,
  };
  assertChatUiTiming(timing);
  return timing;
}

export function markTerminalRendered(
  timing: ChatUiTiming,
  handlerStartedAtMs: number,
  terminalRenderedAtMs: number,
): void {
  const terminalMs = elapsedBetween(handlerStartedAtMs, terminalRenderedAtMs) ?? 0;
  timing.terminal_rendered_ms = terminalMs;
  timing.total_pipeline_ms = terminalMs;
  timing.post_processing_ms =
    timing.visible_response_ms === null
      ? 0
      : Math.max(0, terminalMs - timing.visible_response_ms);
  assertChatUiTiming(timing);
}

export function assertChatUiTiming(timing: ChatUiTiming): void {
  const values = [
    timing.answer_streaming_ms,
    timing.total_pipeline_ms,
    timing.post_processing_ms,
    timing.terminal_rendered_ms,
    ...(timing.first_token_ms === null ? [] : [timing.first_token_ms]),
    ...(timing.visible_response_ms === null ? [] : [timing.visible_response_ms]),
  ];
  if (values.some((value) => !Number.isFinite(value) || value < 0)) {
    throw new Error('Chat timing contains a negative or non-finite duration.');
  }
  if (
    timing.first_token_ms !== null &&
    timing.visible_response_ms !== null &&
    timing.first_token_ms > timing.visible_response_ms
  ) {
    throw new Error('Chat first-token timing exceeds the visible response duration.');
  }
  if (
    timing.visible_response_ms !== null &&
    timing.visible_response_ms > timing.total_pipeline_ms
  ) {
    throw new Error('Chat visible response timing exceeds the total pipeline duration.');
  }
  if (
    timing.visible_response_ms !== null &&
    timing.answer_streaming_ms > timing.visible_response_ms
  ) {
    throw new Error('Chat answer streaming timing exceeds the visible response duration.');
  }
}

export function timingToleranceMs(referenceMs: number): number {
  return Math.max(250, safeDuration(referenceMs) * 0.1);
}

export function timingWithinTolerance(
  displayedMs: number,
  referenceMs: number,
): boolean {
  return Math.abs(safeDuration(displayedMs) - safeDuration(referenceMs)) <=
    timingToleranceMs(referenceMs);
}

export function nanosecondsToMilliseconds(value: bigint): number {
  if (value < 0n) {
    throw new Error('Nanosecond duration cannot be negative.');
  }
  const milliseconds = value / 1_000_000n;
  return Number(milliseconds > BigInt(Number.MAX_SAFE_INTEGER)
    ? BigInt(Number.MAX_SAFE_INTEGER)
    : milliseconds);
}

export function microsecondsToMilliseconds(value: number): number {
  return safeDuration(value) / 1_000;
}

export function safeDuration(value: number): number {
  if (!Number.isFinite(value) || value < 0) {
    throw new Error('Duration must be finite and non-negative.');
  }
  return Math.min(value, Number.MAX_SAFE_INTEGER);
}

function elapsedBetween(
  startedAtMs: number | undefined,
  completedAtMs: number | undefined,
): number | null {
  if (startedAtMs === undefined || completedAtMs === undefined) {
    return null;
  }
  return safeDuration(completedAtMs - startedAtMs);
}
