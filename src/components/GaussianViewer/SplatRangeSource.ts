export const SPLAT_RANGE_THRESHOLD_BYTES = 256 * 1024 * 1024;

// Tauri's asset protocol caps a single byte-range response at 1000 KiB.
export const SPLAT_RANGE_CHUNK_BYTES = 1000 * 1024;

export type SplatRangeErrorCode =
  | "invalid-size"
  | "range-unsupported"
  | "invalid-content-range"
  | "file-size-changed"
  | "invalid-chunk";

export class SplatRangeError extends Error {
  readonly code: SplatRangeErrorCode;

  constructor(code: SplatRangeErrorCode, message: string) {
    super(message);
    this.name = "SplatRangeError";
    this.code = code;
  }
}

export interface SplatRangeSource {
  response: Response;
  abort: () => void;
  readonly aborted: boolean;
}

type FetchRange = typeof fetch;

const contentRangePattern = /^bytes\s+(\d+)-(\d+)\/(\d+)$/i;

export function shouldUseSplatRange(fileSize: number) {
  return Number.isSafeInteger(fileSize) && fileSize > SPLAT_RANGE_THRESHOLD_BYTES;
}

function abortedError(signal: AbortSignal) {
  return signal.reason instanceof Error
    ? signal.reason
    : new DOMException("Large PLY loading was cancelled.", "AbortError");
}

export function createSplatRangeSource(
  url: string,
  fileSize: number,
  fetchRange: FetchRange = fetch,
): SplatRangeSource {
  if (!Number.isSafeInteger(fileSize) || fileSize <= 0) {
    throw new SplatRangeError("invalid-size", `Invalid PLY file size: ${fileSize}.`);
  }

  const abortController = new AbortController();
  let offset = 0;
  let complete = false;

  const stream = new ReadableStream<Uint8Array>({
    async pull(controller) {
      if (complete) return;
      if (abortController.signal.aborted) throw abortedError(abortController.signal);
      if (offset === fileSize) {
        complete = true;
        controller.close();
        return;
      }

      const start = offset;
      const requestedEnd = Math.min(start + SPLAT_RANGE_CHUNK_BYTES - 1, fileSize - 1);
      const response = await fetchRange(url, {
        method: "GET",
        headers: { Range: `bytes=${start}-${requestedEnd}` },
        cache: "no-store",
        signal: abortController.signal,
      });

      if (response.status !== 206) {
        throw new SplatRangeError(
          "range-unsupported",
          `Large PLY range request returned HTTP ${response.status}; expected 206 Partial Content.`,
        );
      }

      const contentRange = response.headers.get("content-range")?.trim() ?? "";
      const match = contentRangePattern.exec(contentRange);
      if (!match) {
        throw new SplatRangeError(
          "invalid-content-range",
          "Large PLY range response did not include a valid Content-Range header.",
        );
      }

      const actualStart = Number(match[1]);
      const actualEnd = Number(match[2]);
      const actualTotal = Number(match[3]);
      if (actualTotal !== fileSize) {
        throw new SplatRangeError(
          "file-size-changed",
          `The PLY file size changed while loading (${actualTotal} instead of ${fileSize} bytes).`,
        );
      }
      if (actualStart !== start || actualEnd !== requestedEnd || actualEnd < actualStart) {
        throw new SplatRangeError(
          "invalid-content-range",
          `Unexpected PLY byte range ${contentRange}; requested bytes ${start}-${requestedEnd}.`,
        );
      }

      const bytes = new Uint8Array(await response.arrayBuffer());
      const expectedLength = actualEnd - actualStart + 1;
      if (bytes.byteLength === 0 || bytes.byteLength !== expectedLength) {
        throw new SplatRangeError(
          "invalid-chunk",
          `PLY byte range ${actualStart}-${actualEnd} returned ${bytes.byteLength} of ${expectedLength} bytes.`,
        );
      }

      offset = actualEnd + 1;
      controller.enqueue(bytes);
      if (offset === fileSize) {
        complete = true;
        controller.close();
      }
    },
    cancel() {
      abortController.abort();
    },
  });

  return {
    response: new Response(stream, {
      headers: {
        "Content-Length": String(fileSize),
        "Content-Type": "application/octet-stream",
      },
    }),
    abort: () => abortController.abort(),
    get aborted() {
      return abortController.signal.aborted;
    },
  };
}
