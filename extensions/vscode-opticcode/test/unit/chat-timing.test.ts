import * as assert from 'node:assert/strict';
import { describe, it } from 'node:test';

import {
  assertChatUiTiming,
  createChatUiTiming,
  markTerminalRendered,
  microsecondsToMilliseconds,
  nanosecondsToMilliseconds,
  timingWithinTolerance,
} from '../../src/chat/timing';

describe('OpticCode chat timing', () => {
  it('separates visible response from a long terminal post-processing phase', () => {
    const timing = createChatUiTiming('request-timing', 100, 400, 3_100, 23_100);
    assert.equal(timing.first_token_ms, 300);
    assert.equal(timing.answer_streaming_ms, 2_700);
    assert.equal(timing.visible_response_ms, 3_000);
    assert.equal(timing.total_pipeline_ms, 23_000);
    assert.equal(timing.post_processing_ms, 20_000);
    markTerminalRendered(timing, 100, 23_150);
    assert.equal(timing.total_pipeline_ms, 23_050);
    assert.equal(timing.post_processing_ms, 20_050);
  });

  it('uses explicit and bounded nanosecond, microsecond, and millisecond conversions', () => {
    assert.equal(nanosecondsToMilliseconds(3_500_000_000n), 3_500);
    assert.equal(microsecondsToMilliseconds(3_500_000), 3_500);
    assert.throws(() => nanosecondsToMilliseconds(-1n), /negative/u);
    assert.throws(() => microsecondsToMilliseconds(Number.NaN), /finite/u);
  });

  it('enforces timing order and the 250 ms or ten-percent tolerance', () => {
    assert.equal(timingWithinTolerance(4_100, 4_000), true);
    assert.equal(timingWithinTolerance(4_500, 4_000), false);
    assert.throws(
      () => assertChatUiTiming({
        schema_version: 1,
        request_id: 'bad-order',
        clock: 'performance.now',
        first_token_ms: 500,
        answer_streaming_ms: 100,
        visible_response_ms: 400,
        total_pipeline_ms: 450,
        post_processing_ms: 50,
        terminal_rendered_ms: 450,
      }),
      /first-token/u,
    );
  });
});
