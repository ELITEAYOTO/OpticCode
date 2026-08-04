import { spawn, type ChildProcessWithoutNullStreams, type SpawnOptionsWithoutStdio } from 'node:child_process';
import { randomUUID } from 'node:crypto';

import { OpticCodeClientError } from './errors';
import type {
  AssistantProtocolEvent,
  AssistantStreamResult,
  CancellationLike,
  GenerationResult,
  JsonObject,
} from './types';
import { isRecord, validateAssistantEvent } from './validation';

const DEFAULT_JSON_LIMIT = 16 * 1024 * 1024;
const DEFAULT_NDJSON_LINE_LIMIT = 18 * 1024 * 1024;
const DEFAULT_NDJSON_TOTAL_LIMIT = 64 * 1024 * 1024;
const DEFAULT_STDERR_LIMIT = 1024 * 1024;
const DEFAULT_EVENT_LIMIT = 16_384;
const CANCELLATION_GRACE_MS = 1_000;

export interface ClientLogger {
  append(value: string): void;
}

export interface ClientLimits {
  jsonBytes: number;
  ndjsonLineBytes: number;
  ndjsonTotalBytes: number;
  stderrBytes: number;
  events: number;
}

export interface ProtocolClientOptions {
  executablePath: string;
  workingDirectory: string;
  timeoutMs: number;
  logger?: ClientLogger;
  debug?: boolean;
  prefixArguments?: string[];
  limits?: Partial<ClientLimits>;
  spawnAdapter?: SpawnAdapter;
}

export interface SpawnInvocation {
  executablePath: string;
  arguments: readonly string[];
  options: SpawnOptionsWithoutStdio;
}

export type SpawnAdapter = (
  executablePath: string,
  args: readonly string[],
  options: SpawnOptionsWithoutStdio,
) => ChildProcessWithoutNullStreams;

interface RawProcessResult {
  stdout: Buffer;
  stderr: string;
  exitCode: number | null;
  signal: NodeJS.Signals | null;
  durationMs: number;
}

export function createSpawnInvocation(
  executablePath: string,
  args: readonly string[],
  workingDirectory: string,
): SpawnInvocation {
  return {
    executablePath,
    arguments: [...args],
    options: {
      cwd: workingDirectory,
      shell: false,
      windowsHide: true,
      stdio: ['pipe', 'pipe', 'pipe'],
      env: { ...process.env, NO_COLOR: '1' },
    },
  };
}

export function createRequestId(command: 'ask' | 'plan'): string {
  return `vscode-${command}-${randomUUID()}`;
}

export class OpticCodeProtocolClient {
  private readonly limits: ClientLimits;
  private readonly spawnAdapter: SpawnAdapter;

  public constructor(private readonly options: ProtocolClientOptions) {
    this.limits = {
      jsonBytes: options.limits?.jsonBytes ?? DEFAULT_JSON_LIMIT,
      ndjsonLineBytes: options.limits?.ndjsonLineBytes ?? DEFAULT_NDJSON_LINE_LIMIT,
      ndjsonTotalBytes: options.limits?.ndjsonTotalBytes ?? DEFAULT_NDJSON_TOTAL_LIMIT,
      stderrBytes: options.limits?.stderrBytes ?? DEFAULT_STDERR_LIMIT,
      events: options.limits?.events ?? DEFAULT_EVENT_LIMIT,
    };
    this.spawnAdapter = options.spawnAdapter ?? ((executable, args, spawnOptions) => {
      return spawn(executable, args, spawnOptions) as ChildProcessWithoutNullStreams;
    });
  }

  public async runJson<T extends JsonObject>(
    args: readonly string[],
    validate: (value: unknown) => T,
    cancellation?: CancellationLike,
    acceptNonZero = false,
  ): Promise<T> {
    const result = await this.runRaw(args, cancellation);
    if (result.exitCode !== 0 && !acceptNonZero) {
      throw new OpticCodeClientError(
        'process_failed',
        `OpticCode exited with code ${String(result.exitCode)}.`,
        { exitCode: result.exitCode, stderr: result.stderr },
      );
    }
    let decoded: string;
    try {
      decoded = new TextDecoder('utf-8', { fatal: true }).decode(result.stdout);
    } catch (error) {
      throw new OpticCodeClientError('invalid_json', 'OpticCode stdout is not valid UTF-8.', {
        cause: String(error),
      });
    }
    let value: unknown;
    try {
      value = JSON.parse(decoded);
    } catch (error) {
      throw new OpticCodeClientError('invalid_json', 'OpticCode stdout is not one JSON document.', {
        cause: String(error),
      });
    }
    return validate(value);
  }

  public runJsonObject(
    args: readonly string[],
    cancellation?: CancellationLike,
    acceptNonZero = false,
  ): Promise<JsonObject> {
    return this.runJson(
      args,
      (value) => {
        if (!isRecord(value)) {
          throw new OpticCodeClientError('invalid_json', 'Expected a JSON object from OpticCode.');
        }
        return value;
      },
      cancellation,
      acceptNonZero,
    );
  }

  public async runAssistantStream(
    args: readonly string[],
    requestId: string,
    onEvent?: (event: AssistantProtocolEvent) => void,
    cancellation?: CancellationLike,
  ): Promise<AssistantStreamResult> {
    const fullArguments = [
      ...(this.options.prefixArguments ?? []),
      ...args,
      '--protocol-jsonl',
      '--request-id',
      requestId,
    ];
    const invocation = createSpawnInvocation(
      this.options.executablePath,
      fullArguments,
      this.options.workingDirectory,
    );
    this.debug(`spawn ${JSON.stringify(fullArguments)}`);
    const started = Date.now();
    let child: ChildProcessWithoutNullStreams;
    try {
      child = this.spawnAdapter(
        invocation.executablePath,
        invocation.arguments,
        invocation.options,
      );
    } catch (error) {
      throw new OpticCodeClientError('spawn_failed', 'Failed to start OpticCode.', {
        cause: String(error),
      });
    }

    return await new Promise<AssistantStreamResult>((resolve, reject) => {
      let stdoutBuffer = Buffer.alloc(0);
      let totalStdout = 0;
      let stderr = '';
      let stderrBytes = 0;
      let expectedSequence = 0;
      let terminal: AssistantProtocolEvent | undefined;
      let generation: GenerationResult | undefined;
      let response = '';
      let parserError: OpticCodeClientError | undefined;
      let timedOut = false;
      let cancellationRequested = cancellation?.isCancellationRequested ?? false;
      let forcedTermination = false;
      const events: AssistantProtocolEvent[] = [];
      const nestedSequences = new Map<string, number>();
      const nestedRequestsByContext = new Map<string, string>();
      const nestedTerminals = new Set<string>();

      const forceTerminate = (): void => {
        if (child.exitCode === null && child.signalCode === null) {
          forcedTermination = child.kill();
        }
      };
      const requestCleanCancellation = (): void => {
        cancellationRequested = true;
        if (child.stdin.writable) {
          child.stdin.end('cancel\n');
        }
      };
      if (cancellationRequested) {
        requestCleanCancellation();
      }
      const cancellationDisposable = cancellation?.onCancellationRequested(() => {
        requestCleanCancellation();
        setTimeout(forceTerminate, CANCELLATION_GRACE_MS).unref();
      });
      const timeout = setTimeout(() => {
        timedOut = true;
        requestCleanCancellation();
        setTimeout(forceTerminate, CANCELLATION_GRACE_MS).unref();
      }, this.options.timeoutMs);

      const failParser = (error: OpticCodeClientError): void => {
        if (parserError === undefined) {
          parserError = error;
          forceTerminate();
        }
      };
      const acceptLine = (line: Buffer): void => {
        if (line.length === 0) {
          failParser(new OpticCodeClientError('invalid_ndjson', 'Empty NDJSON line received.'));
          return;
        }
        if (line.length > this.limits.ndjsonLineBytes) {
          failParser(new OpticCodeClientError('output_limit', 'NDJSON line exceeded its byte limit.'));
          return;
        }
        let text: string;
        try {
          text = new TextDecoder('utf-8', { fatal: true }).decode(line);
        } catch (error) {
          failParser(
            new OpticCodeClientError('invalid_ndjson', 'NDJSON line is not valid UTF-8.', {
              cause: String(error),
            }),
          );
          return;
        }
        let raw: unknown;
        try {
          raw = JSON.parse(text);
        } catch (error) {
          failParser(
            new OpticCodeClientError('invalid_ndjson', 'NDJSON line is not valid JSON.', {
              cause: String(error),
            }),
          );
          return;
        }
        let event: AssistantProtocolEvent;
        try {
          event = validateAssistantEvent(raw, requestId);
        } catch (error) {
          failParser(
            error instanceof OpticCodeClientError
              ? error
              : new OpticCodeClientError('invalid_ndjson', String(error)),
          );
          return;
        }
        if (terminal !== undefined) {
          failParser(
            new OpticCodeClientError('terminal_duplicate', 'Event received after terminal event.'),
          );
          return;
        }
        if (event.sequence !== expectedSequence) {
          failParser(
            new OpticCodeClientError(
              'sequence_mismatch',
              `Expected assistant sequence ${expectedSequence}, received ${event.sequence}.`,
            ),
          );
          return;
        }
        expectedSequence += 1;
        if (events.length >= this.limits.events) {
          failParser(new OpticCodeClientError('output_limit', 'Assistant event limit exceeded.'));
          return;
        }
        events.push(event);
        if (event.event !== undefined) {
          const context = event.context_mode ?? 'unknown';
          const activeRequest = nestedRequestsByContext.get(context);
          if (activeRequest !== undefined && activeRequest !== event.event.request_id) {
            failParser(
              new OpticCodeClientError(
                'request_mismatch',
                `LLM request ID changed within ${context} context.`,
              ),
            );
            return;
          }
          nestedRequestsByContext.set(context, event.event.request_id);
          if (nestedTerminals.has(event.event.request_id)) {
            failParser(
              new OpticCodeClientError(
                'terminal_duplicate',
                'LLM event received after its terminal event.',
              ),
            );
            return;
          }
          const expectedNested = nestedSequences.get(event.event.request_id) ?? 0;
          if (event.event.sequence !== expectedNested) {
            failParser(
              new OpticCodeClientError(
                'sequence_mismatch',
                `Expected LLM sequence ${expectedNested}, received ${event.event.sequence}.`,
              ),
            );
            return;
          }
          nestedSequences.set(event.event.request_id, expectedNested + 1);
          if (event.event.type === 'delta') {
            response += event.event.text ?? '';
          } else if (event.event.type === 'completed') {
            generation = event.event.result;
          }
          if (['completed', 'failed', 'cancelled'].includes(event.event.type)) {
            nestedTerminals.add(event.event.request_id);
          }
        }
        if (['completed', 'failed', 'cancelled'].includes(event.type)) {
          terminal = event;
        }
        onEvent?.(event);
      };

      child.stdout.on('data', (chunk: Buffer) => {
        if (parserError !== undefined) {
          return;
        }
        totalStdout += chunk.length;
        if (totalStdout > this.limits.ndjsonTotalBytes) {
          failParser(new OpticCodeClientError('output_limit', 'NDJSON output exceeded its byte limit.'));
          return;
        }
        stdoutBuffer = Buffer.concat([stdoutBuffer, chunk]);
        let newline = stdoutBuffer.indexOf(0x0a);
        while (newline >= 0) {
          let line = stdoutBuffer.subarray(0, newline);
          stdoutBuffer = stdoutBuffer.subarray(newline + 1);
          if (line.at(-1) === 0x0d) {
            line = line.subarray(0, -1);
          }
          acceptLine(line);
          if (parserError !== undefined) {
            return;
          }
          newline = stdoutBuffer.indexOf(0x0a);
        }
        if (stdoutBuffer.length > this.limits.ndjsonLineBytes) {
          failParser(new OpticCodeClientError('output_limit', 'NDJSON line exceeded its byte limit.'));
        }
      });
      child.stderr.on('data', (chunk: Buffer) => {
        this.options.logger?.append(chunk.toString('utf8'));
        if (stderrBytes < this.limits.stderrBytes) {
          const retained = chunk.subarray(0, this.limits.stderrBytes - stderrBytes);
          stderr += retained.toString('utf8');
          stderrBytes += retained.length;
        }
      });
      child.on('error', (error) => {
        clearTimeout(timeout);
        cancellationDisposable?.dispose();
        reject(
          new OpticCodeClientError('spawn_failed', 'Failed to start OpticCode.', {
            cause: String(error),
          }),
        );
      });
      child.on('close', (exitCode, signal) => {
        clearTimeout(timeout);
        cancellationDisposable?.dispose();
        if (parserError !== undefined) {
          reject(parserError);
          return;
        }
        if (stdoutBuffer.length !== 0) {
          reject(
            new OpticCodeClientError('invalid_ndjson', 'NDJSON stream ended with a partial line.'),
          );
          return;
        }
        if (timedOut) {
          reject(
            new OpticCodeClientError('timeout', 'OpticCode exceeded the configured timeout.', {
              exitCode,
              signal,
            }),
          );
          return;
        }
        if (terminal === undefined) {
          const code = cancellationRequested
            ? 'cancellation_unconfirmed'
            : signal !== null || forcedTermination || (exitCode !== null && exitCode !== 0)
              ? 'process_interrupted'
              : 'terminal_missing';
          reject(
            new OpticCodeClientError(
              code,
              cancellationRequested
                ? 'OpticCode stopped without confirming cancellation.'
                : 'OpticCode stopped without a terminal protocol event.',
              { exitCode, signal },
            ),
          );
          return;
        }
        const incompleteNested = [...nestedSequences.keys()].filter(
          (nestedRequest) => !nestedTerminals.has(nestedRequest),
        );
        if (incompleteNested.length !== 0) {
          reject(
            new OpticCodeClientError(
              'terminal_missing',
              'One or more nested LLM streams ended without a terminal event.',
              { requestIds: incompleteNested },
            ),
          );
          return;
        }
        const status = terminal.type;
        if (status !== 'completed' && status !== 'failed' && status !== 'cancelled') {
          reject(
            new OpticCodeClientError('terminal_missing', 'Invalid terminal event state.'),
          );
          return;
        }
        if (status === 'completed' && exitCode !== 0) {
          reject(
            new OpticCodeClientError(
              'process_failed',
              `OpticCode completed the protocol but exited with code ${String(exitCode)}.`,
            ),
          );
          return;
        }
        resolve({
          requestId,
          status,
          response,
          events,
          terminal,
          summary: terminal.summary,
          generation,
          durationMs: Date.now() - started,
          exitCode,
          stderr,
          cancellationConfirmed: status === 'cancelled' && cancellationRequested,
        });
      });
    });
  }

  private async runRaw(
    args: readonly string[],
    cancellation?: CancellationLike,
  ): Promise<RawProcessResult> {
    const fullArguments = [...(this.options.prefixArguments ?? []), ...args];
    const invocation = createSpawnInvocation(
      this.options.executablePath,
      fullArguments,
      this.options.workingDirectory,
    );
    this.debug(`spawn ${JSON.stringify(fullArguments)}`);
    const started = Date.now();
    let child: ChildProcessWithoutNullStreams;
    try {
      child = this.spawnAdapter(
        invocation.executablePath,
        invocation.arguments,
        invocation.options,
      );
    } catch (error) {
      throw new OpticCodeClientError('spawn_failed', 'Failed to start OpticCode.', {
        cause: String(error),
      });
    }

    return await new Promise<RawProcessResult>((resolve, reject) => {
      const stdout: Buffer[] = [];
      let stdoutBytes = 0;
      let stderr = '';
      let stderrBytes = 0;
      let limitExceeded = false;
      let timedOut = false;
      let cancelled = cancellation?.isCancellationRequested ?? false;
      const terminate = (): void => {
        if (child.exitCode === null && child.signalCode === null) {
          child.kill();
        }
      };
      if (cancelled) {
        terminate();
      }
      const cancellationDisposable = cancellation?.onCancellationRequested(() => {
        cancelled = true;
        terminate();
      });
      const timeout = setTimeout(() => {
        timedOut = true;
        terminate();
      }, this.options.timeoutMs);
      child.stdout.on('data', (chunk: Buffer) => {
        stdoutBytes += chunk.length;
        if (stdoutBytes > this.limits.jsonBytes) {
          limitExceeded = true;
          terminate();
        } else {
          stdout.push(chunk);
        }
      });
      child.stderr.on('data', (chunk: Buffer) => {
        this.options.logger?.append(chunk.toString('utf8'));
        if (stderrBytes < this.limits.stderrBytes) {
          const retained = chunk.subarray(0, this.limits.stderrBytes - stderrBytes);
          stderr += retained.toString('utf8');
          stderrBytes += retained.length;
        }
      });
      child.on('error', (error) => {
        clearTimeout(timeout);
        cancellationDisposable?.dispose();
        reject(
          new OpticCodeClientError('spawn_failed', 'Failed to start OpticCode.', {
            cause: String(error),
          }),
        );
      });
      child.on('close', (exitCode, signal) => {
        clearTimeout(timeout);
        cancellationDisposable?.dispose();
        if (limitExceeded) {
          reject(new OpticCodeClientError('output_limit', 'JSON output exceeded its byte limit.'));
        } else if (timedOut) {
          reject(new OpticCodeClientError('timeout', 'OpticCode exceeded the configured timeout.'));
        } else if (cancelled) {
          reject(
            new OpticCodeClientError(
              'cancellation_unconfirmed',
              'Non-streaming process was interrupted; provider cancellation was not confirmed.',
            ),
          );
        } else {
          resolve({
            stdout: Buffer.concat(stdout),
            stderr,
            exitCode,
            signal,
            durationMs: Date.now() - started,
          });
        }
      });
    });
  }

  private debug(message: string): void {
    if (this.options.debug === true) {
      this.options.logger?.append(`[debug] ${message}\n`);
    }
  }
}
