/**
 * The types a bespoke operation implementation exchanges with the generated
 * glue. Data only: no I/O, no transport, no dependency on the HTTP runtime, so
 * an operation implemented over a legacy library, a cache, or a crypto module
 * pulls in nothing it does not use.
 */

/**
 * What a raw-form implementation returns instead of the operation's declared
 * output. The generated glue treats it exactly as it treats a protocol
 * response: on `success` the `body` is decoded strictly into the declared
 * output type, otherwise `code` is matched against the operation's declared
 * error codes with a generic fallback. Returning it costs the implementation no
 * mapping code when the shapes already line up.
 *
 * `body` is the JSON text of the declared output (on success) or of the
 * declared error (on failure). `code` is read only when `success` is false.
 */
export interface Outcome {
  readonly success: boolean;
  readonly code: string;
  readonly body: string;
}
