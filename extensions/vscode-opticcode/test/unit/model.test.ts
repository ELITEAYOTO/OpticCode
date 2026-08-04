import * as assert from 'node:assert/strict';
import { describe, it } from 'node:test';

import {
  assistantMarkdown,
  findingsFromJavaEdits,
  findingsFromJavaSyntax,
  utf16ColumnFromUtf8,
  worktreeMarkdown,
} from '../../src/model';
import type { AssistantStreamResult, JsonObject } from '../../src/protocol/types';

describe('interface model mapping', () => {
  it('converts Tree-sitter UTF-8 byte columns to VS Code UTF-16 units', () => {
    const line = '\u00e9\ud83d\ude00abc';
    assert.equal(utf16ColumnFromUtf8(line, 0), 0);
    assert.equal(utf16ColumnFromUtf8(line, 2), 1);
    assert.equal(utf16ColumnFromUtf8(line, 6), 3);
    assert.equal(utf16ColumnFromUtf8(line, 7), 4);
    assert.equal(utf16ColumnFromUtf8(line, 100), 6);
  });

  it('maps Java diagnostics to valid zero-based ranges', () => {
    const findings = findingsFromJavaSyntax(
      {
        root: 'C:\\project',
        files: [
          {
            path: 'src\\Broken.java',
            diagnostics: [
              {
                kind: 'syntax_error',
                message: 'missing brace',
                node_kind: 'class_body',
                range: {
                  start: { row: 4, column: 8 },
                  end: { row: 4, column: 8 },
                },
              },
            ],
          },
        ],
      },
      'C:\\project',
    );
    assert.equal(findings.length, 1);
    assert.deepEqual(findings[0]?.range, {
      start: { line: 4, character: 8 },
      end: { line: 4, character: 9 },
    });
  });

  it('maps accepted and refused edit findings without applying them', () => {
    const report: JsonObject = {
      root: 'C:\\project',
      proposals: [
        {
          id: 'edit-1',
          file: 'Plugin.java',
          rule_id: 'MC18-MATERIAL-001',
          expected_content: 'GUNPOWDER',
          replacement: 'SULPHUR',
          edit_range: { start: { row: 2, column: 4 }, end: { row: 2, column: 13 } },
          reason: 'Bukkit 1.8.8 name',
          confidence: 'syntax_exact',
        },
      ],
      rejections: [
        { file: 'Other.java', kind: 'ambiguous', message: 'owner is ambiguous' },
      ],
    };
    const findings = findingsFromJavaEdits(report, 'C:\\project');
    assert.equal(findings.length, 2);
    assert.equal(findings[0]?.decision, 'accepted');
    assert.equal(findings[1]?.decision, 'refused');
  });

  it('renders Ask metadata and referenced files without snippet content', () => {
    const result: AssistantStreamResult = {
      requestId: 'ask-1',
      status: 'completed',
      response: 'Use Material.SULPHUR.',
      events: [],
      terminal: {
        schema_version: 1,
        protocol: 'opticcode.assistant',
        request_id: 'ask-1',
        sequence: 0,
        type: 'completed',
      },
      summary: {
        command: 'ask',
        success: true,
        model: 'qwen',
        requested_context_mode: 'symbol',
        used_context_mode: 'symbol',
        preparation_duration_us: 10,
        warnings: ['bounded context'],
        context_files: [
          { context_mode: 'symbol', path: 'src/Plugin.java', snippets: 1 },
        ],
        runs: [
          {
            context_mode: 'symbol',
            generated: true,
            estimated_prompt_tokens: 100,
            prompt_tokens: 90,
            generated_tokens: 5,
          },
        ],
      },
      durationMs: 1500,
      exitCode: 0,
      stderr: '',
      cancellationConfirmed: false,
    };
    const markdown = assistantMarkdown('Answer', result);
    assert.match(markdown, /Material\.SULPHUR/);
    assert.match(markdown, /src\/Plugin\.java/);
    assert.match(markdown, /Estimated prompt tokens \| 100/);
    assert.doesNotMatch(markdown, /snippet content/);
  });

  it('separates edit, build, diff, and cleanup worktree results', () => {
    const markdown = worktreeMarkdown({
      status: 'passed',
      cleanup_success: false,
      lease_recovery_required: true,
      materialization: { success: true },
      worktree: {
        build: { success: true },
        diff: { files: 1 },
        cleanup: { success: false },
      },
    });
    for (const heading of ['Edit Result', 'Build Result', 'Diff', 'Cleanup Result']) {
      assert.match(markdown, new RegExp(`## ${heading}`));
    }
  });
});
