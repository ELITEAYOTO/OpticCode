import * as assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import * as path from 'node:path';
import { describe, it } from 'node:test';

import { CHAT_COMMANDS, CHAT_PARTICIPANT_ID } from '../../src/chat/model';

interface ManifestParticipant {
  id?: unknown;
  name?: unknown;
  fullName?: unknown;
  isSticky?: unknown;
  commands?: Array<{ name?: unknown }>;
}

describe('OpticCode chat manifest', () => {
  it('contributes one stable participant and every documented slash command', () => {
    const manifestPath = path.resolve(__dirname, '../../../package.json');
    const manifest = JSON.parse(readFileSync(manifestPath, 'utf8')) as {
      activationEvents?: unknown[];
      enabledApiProposals?: unknown;
      contributes?: { chatParticipants?: ManifestParticipant[] };
    };
    const participants = manifest.contributes?.chatParticipants ?? [];
    const participant = participants.find((candidate) => candidate.id === CHAT_PARTICIPANT_ID);
    assert.ok(participant);
    assert.equal(participant.name, 'opticcode');
    assert.equal(participant.fullName, 'OpticCode Local');
    assert.equal(participant.isSticky, true);
    assert.deepEqual(
      participant.commands?.map((command) => command.name),
      CHAT_COMMANDS,
    );
    assert.ok(manifest.activationEvents?.includes(`onChatParticipant:${CHAT_PARTICIPANT_ID}`));
    assert.equal(manifest.enabledApiProposals, undefined);
  });
});
