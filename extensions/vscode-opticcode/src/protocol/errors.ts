export type ClientErrorCode =
  | 'executable_not_found'
  | 'spawn_failed'
  | 'invalid_json'
  | 'invalid_ndjson'
  | 'protocol_incompatible'
  | 'sequence_mismatch'
  | 'request_mismatch'
  | 'terminal_missing'
  | 'terminal_duplicate'
  | 'output_limit'
  | 'timeout'
  | 'process_failed'
  | 'process_interrupted'
  | 'cancellation_unconfirmed';

export class OpticCodeClientError extends Error {
  public constructor(
    public readonly code: ClientErrorCode,
    message: string,
    public readonly details: Record<string, unknown> = {},
  ) {
    super(message);
    this.name = 'OpticCodeClientError';
  }
}
