import { spawn, type ChildProcessWithoutNullStreams, type SpawnOptionsWithoutStdio } from 'node:child_process';
import { randomUUID } from 'node:crypto';

import { OpticCodeClientError } from './errors';
import type {
  AssistantProtocolEvent,
  AssistantStreamResult,
  CancellationLike,
  ChatProtocolEvent,
  ChatProtocolRequest,
  ChatStreamResult,
  GenerationResult,
  JsonObject,
} from './types';
import { isRecord, validateAssistantEvent, validateChatEvent } from './validation';

const DEFAULT_JSON_LIMIT = 16 * 1024 * 1024;
const DEFAULT_NDJSON_LINE_LIMIT = 18 * 1024 * 1024;
const DEFAULT_NDJSON_TOTAL_LIMIT = 64 * 1024 * 1024;
const DEFAULT_STDERR_LIMIT = 1024 * 1024;
const DEFAULT_EVENT_LIMIT = 16_384;
const CHAT_REQUEST_LIMIT = 2 * 1024 * 1024;
const CANCELLATION_GRACE_MS = 1_000;

function isTerminalType(type: string): type is 'completed' | 'failed' | 'cancelled' {
  return type === 'completed' || type === 'failed' || type === 'cancelled';
}

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

interface SequencedProtocolEvent extends JsonObject {
  sequence: number;
  type: string;
}

interface SequencedStreamResult<TEvent extends SequencedProtocolEvent> {
  events: TEvent[];
  terminal: TEvent;
  durationMs: number;
  exitCode: number | null;
  stderr: string;
  cancellationConfirmed: boolean;
}

interface SequencedStreamOptions<TEvent extends SequencedProtocolEvent> {
  arguments: readonly string[];
  requestId: string;
  protocolName: string;
  validate(value: unknown): TEvent;
  inspect?(event: TEvent): void;
  onEvent?(event: TEvent): void;
  initialInput?: string;
  cancellationInput: string;
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
    let response = '';
    let generation: GenerationResult | undefined;
    const nestedSequences = new Map<string, number>();
    const nestedRequestsByContext = new Map<string, string>();
    const nestedTerminals = new Set<string>();
    const stream = await this.runSequencedStream<AssistantProtocolEvent>(
      {
        arguments: [
          ...(this.options.prefixArguments ?? []),
          ...args,
          '--protocol-jsonl',
          '--request-id',
          requestId,
        ],
        requestId,
        protocolName: 'assistant',
        validate: (value) => validateAssistantEvent(value, requestId),
        inspect: (event) => {
          if (event.event === undefined) {
            return;
          }
          const context = event.context_mode ?? 'unknown';
          const activeRequest = nestedRequestsByContext.get(context);
          if (activeRequest !== undefined && activeRequest !== event.event.request_id) {
            throw new OpticCodeClientError(
              'request_mismatch',
              `LLM request ID changed within ${context} context.`,
            );
          }
          nestedRequestsByContext.set(context, event.event.request_id);
          if (nestedTerminals.has(event.event.request_id)) {
            throw new OpticCodeClientError(
              'terminal_duplicate',
              'LLM event received after its terminal event.',
            );
          }
          const expectedNested = nestedSequences.get(event.event.request_id) ?? 0;
          if (event.event.sequence !== expectedNested) {
            throw new OpticCodeClientError(
              'sequence_mismatch',
              `Expected LLM sequence ${expectedNested}, received ${event.event.sequence}.`,
            );
          }
          nestedSequences.set(event.event.request_id, expectedNested + 1);
          if (event.event.type === 'delta') {
            response += event.event.text ?? '';
          } else if (event.event.type === 'completed') {
            generation = event.event.result;
          }
          if (isTerminalType(event.event.type)) {
            nestedTerminals.add(event.event.request_id);
          }
        },
        ...(onEvent === undefined ? {} : { onEvent }),
        cancellationInput: 'cancel\n',
      },
      cancellation,
    );
    const incompleteNested = [...nestedSequences.keys()].filter(
      (nestedRequest) => !nestedTerminals.has(nestedRequest),
    );
    if (incompleteNested.length !== 0) {
      throw new OpticCodeClientError(
        'terminal_missing',
        'One or more nested LLM streams ended without a terminal event.',
        { requestIds: incompleteNested },
      );
    }
    if (!isTerminalType(stream.terminal.type)) {
      throw new OpticCodeClientError('terminal_missing', 'Invalid assistant terminal state.');
    }
    return {
      requestId,
      status: stream.terminal.type,
      response,
      events: stream.events,
      terminal: stream.terminal,
      summary: stream.terminal.summary,
      generation,
      durationMs: stream.durationMs,
      exitCode: stream.exitCode,
      stderr: stream.stderr,
      cancellationConfirmed: stream.cancellationConfirmed,
    };
  }

  public async runChatStream(
    args: readonly string[],
    request: ChatProtocolRequest,
    onEvent?: (event: ChatProtocolEvent) => void,
    cancellation?: CancellationLike,
  ): Promise<ChatStreamResult> {
    let response = '';
    let serialized: string | undefined;
    try {
      serialized = JSON.stringify(request);
    } catch (error) {
      throw new OpticCodeClientError('invalid_json', 'Chat request cannot be serialized.', {
        cause: String(error),
      });
    }
    if (serialized === undefined) {
      throw new OpticCodeClientError('invalid_json', 'Chat request cannot be serialized.');
    }
    const control = JSON.stringify({
      schema_version: 1,
      protocol: 'opticcode.chat.control',
      request_id: request.request_id,
      type: 'cancel',
    });
    const stream = await this.runSequencedStream<ChatProtocolEvent>(
      {
        arguments: [
          ...(this.options.prefixArguments ?? []),
          ...args,
          '--protocol-jsonl',
        ],
        requestId: request.request_id,
        protocolName: 'chat',
        validate: (value) => validateChatEvent(value, request.request_id),
        inspect: (event) => {
          if (event.type === 'token_delta') {
            response += event.text;
          }
        },
        ...(onEvent === undefined ? {} : { onEvent }),
        initialInput: `${serialized}\n`,
        cancellationInput: `${control}\n`,
      },
      cancellation,
    );
    if (!isTerminalType(stream.terminal.type)) {
      throw new OpticCodeClientError('terminal_missing', 'Invalid chat terminal state.');
    }
    return {
      requestId: request.request_id,
      status: stream.terminal.type,
      response,
      events: stream.events,
      terminal: stream.terminal,
      summary: stream.terminal.type === 'completed' ? stream.terminal.summary : undefined,
      durationMs: stream.durationMs,
      exitCode: stream.exitCode,
      stderr: stream.stderr,
      cancellationConfirmed: stream.cancellationConfirmed,
    };
  }

  private async runSequencedStream<TEvent extends SequencedProtocolEvent>(
    stream: SequencedStreamOptions<TEvent>,
    cancellation?: CancellationLike,
  ): Promise<SequencedStreamResult<TEvent>> {
    if (
      stream.initialInput !== undefined &&
      Buffer.byteLength(stream.initialInput, 'utf8') >
        Math.min(this.limits.ndjsonLineBytes, CHAT_REQUEST_LIMIT)
    ) {
      throw new OpticCodeClientError('input_limit', 'Initial protocol request exceeded its byte limit.');
    }
    const invocation = createSpawnInvocation(
      this.options.executablePath,
      stream.arguments,
      this.options.workingDirectory,
    );
    this.debug(`spawn ${JSON.stringify(stream.arguments)}`);
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

    return await new Promise<SequencedStreamResult<TEvent>>((resolve, reject) => {
      let stdoutBuffer = Buffer.alloc(0);
      let totalStdout = 0;
      let stderr = '';
      let stderrBytes = 0;
      let expectedSequence = 0;
      let terminal: TEvent | undefined;
      let parserError: OpticCodeClientError | undefined;
      let timedOut = false;
      let cancellationRequested = cancellation?.isCancellationRequested ?? false;
      let cancellationSent = false;
      let forcedTermination = false;
      let forceTimer: NodeJS.Timeout | undefined;
      const events: TEvent[] = [];

      const forceTerminate = (): void => {
        if (child.exitCode === null && child.signalCode === null) {
          forcedTermination = child.kill() || forcedTermination;
        }
      };
      const scheduleForcedTermination = (): void => {
        if (forceTimer === undefined) {
          forceTimer = setTimeout(forceTerminate, CANCELLATION_GRACE_MS);
          forceTimer.unref();
        }
      };
      const requestCleanCancellation = (): void => {
        cancellationRequested = true;
        if (!cancellationSent && child.stdin.writable) {
          cancellationSent = true;
          child.stdin.end(stream.cancellationInput);
        }
      };
      const failParser = (error: OpticCodeClientError): void => {
        if (parserError === undefined) {
          parserError = error;
          forceTerminate();
        }
      };
      const cleanup = (): void => {
        clearTimeout(timeout);
        if (forceTimer !== undefined) {
          clearTimeout(forceTimer);
        }
        cancellationDisposable?.dispose();
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
        let decoded: string;
        try {
          decoded = new TextDecoder('utf-8', { fatal: true }).decode(line);
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
          raw = JSON.parse(decoded);
        } catch (error) {
          failParser(
            new OpticCodeClientError('invalid_ndjson', 'NDJSON line is not valid JSON.', {
              cause: String(error),
            }),
          );
          return;
        }
        let event: TEvent;
        try {
          event = stream.validate(raw);
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
              `Expected ${stream.protocolName} sequence ${expectedSequence}, received ${event.sequence}.`,
            ),
          );
          return;
        }
        if (events.length >= this.limits.events) {
          failParser(
            new OpticCodeClientError(
              'output_limit',
              `${stream.protocolName} event limit exceeded.`,
            ),
          );
          return;
        }
        try {
          stream.inspect?.(event);
        } catch (error) {
          failParser(
            error instanceof OpticCodeClientError
              ? error
              : new OpticCodeClientError('invalid_ndjson', 'Protocol event inspection failed.', {
                  cause: String(error),
                }),
          );
          return;
        }
        expectedSequence += 1;
        events.push(event);
        if (isTerminalType(event.type)) {
          terminal = event;
          if (child.stdin.writable) {
            child.stdin.end();
          }
        }
        try {
          stream.onEvent?.(event);
        } catch (error) {
          failParser(
            new OpticCodeClientError('process_interrupted', 'Protocol event consumer failed.', {
              cause: String(error),
            }),
          );
        }
      };

      child.stdout.on('data', (chunk: Buffer) => {
        if (parserError !== undefined) {
          return;
        }
        totalStdout += chunk.length;
        if (totalStdout > this.limits.ndjsonTotalBytes) {
          failParser(
            new OpticCodeClientError('output_limit', 'NDJSON output exceeded its byte limit.'),
          );
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
      child.stdin.on('error', (error) => {
        if (
          parserError === undefined &&
          terminal === undefined &&
          !cancellationRequested &&
          child.exitCode === null
        ) {
          failParser(
            new OpticCodeClientError('process_interrupted', 'Failed to write protocol stdin.', {
              cause: String(error),
            }),
          );
        }
      });
      child.on('error', (error) => {
        cleanup();
        reject(
          new OpticCodeClientError('spawn_failed', 'Failed to start OpticCode.', {
            cause: String(error),
          }),
        );
      });
      child.on('close', (exitCode, signal) => {
        cleanup();
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
            ? forcedTermination
              ? 'cancellation_forced'
              : 'cancellation_unconfirmed'
            : signal !== null || forcedTermination || (exitCode !== null && exitCode !== 0)
              ? 'process_interrupted'
              : 'terminal_missing';
          reject(
            new OpticCodeClientError(
              code,
              cancellationRequested
                ? forcedTermination
                  ? 'OpticCode required forced termination after cancellation.'
                  : 'OpticCode stopped without confirming cancellation.'
                : 'OpticCode stopped without a terminal protocol event.',
              { exitCode, signal },
            ),
          );
          return;
        }
        if (terminal.type === 'completed' && exitCode !== 0) {
          reject(
            new OpticCodeClientError(
              'process_failed',
              `OpticCode completed the protocol but exited with code ${String(exitCode)}.`,
            ),
          );
          return;
        }
        resolve({
          events,
          terminal,
          durationMs: Date.now() - started,
          exitCode,
          stderr,
          cancellationConfirmed: terminal.type === 'cancelled' && cancellationRequested,
        });
      });

      if (stream.initialInput !== undefined) {
        child.stdin.write(stream.initialInput);
      }
      if (cancellationRequested) {
        requestCleanCancellation();
        scheduleForcedTermination();
      }
      const cancellationDisposable = cancellation?.onCancellationRequested(() => {
        requestCleanCancellation();
        scheduleForcedTermination();
      });
      const timeout = setTimeout(() => {
        timedOut = true;
        requestCleanCancellation();
        scheduleForcedTermination();
      }, this.options.timeoutMs);
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
