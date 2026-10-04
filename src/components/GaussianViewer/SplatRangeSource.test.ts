import { describe, expect, it, vi } from "vitest";
import {
  SPLAT_RANGE_CHUNK_BYTES,
  SPLAT_RANGE_THRESHOLD_BYTES,
  createSplatRangeSource,
  shouldUseSplatRange,
} from "./SplatRangeSource";

function partialResponse(bytes: Uint8Array, start: number, total: number) {
  const end = start + bytes.byteLength - 1;
  const body = bytes.buffer.slice(bytes.byteOffset, bytes.byteOffset + bytes.byteLength) as ArrayBuffer;
  return new Response(body, {
    status: 206,
    headers: { "Content-Range": `bytes ${start}-${end}/${total}` },
  });
}

describe("large PLY range loading", () => {
  it("only enables ranges above 256 MiB", () => {
    expect(shouldUseSplatRange(SPLAT_RANGE_THRESHOLD_BYTES)).toBe(false);
    expect(shouldUseSplatRange(SPLAT_RANGE_THRESHOLD_BYTES + 1)).toBe(true);
    expect(shouldUseSplatRange(0)).toBe(false);
    expect(shouldUseSplatRange(Number.MAX_SAFE_INTEGER + 1)).toBe(false);
  });

  it("reads sequential chunks without changing the preview URL", async () => {
    const total = SPLAT_RANGE_CHUNK_BYTES * 2 + 17;
    const sourceBytes = new Uint8Array(total);
    sourceBytes.forEach((_, index) => { sourceBytes[index] = index % 251; });
    const ranges: string[] = [];
    const urls: string[] = [];
    let active = 0;
    let maximumActive = 0;
    const fetchRangeMock = vi.fn(async (input: string | URL | Request, init?: RequestInit) => {
      active += 1;
      maximumActive = Math.max(maximumActive, active);
      try {
        urls.push(String(input));
        const range = new Headers(init?.headers).get("range") ?? "";
        ranges.push(range);
        const match = /^bytes=(\d+)-(\d+)$/.exec(range)!;
        const start = Number(match[1]);
        const end = Number(match[2]);
        return partialResponse(sourceBytes.slice(start, end + 1), start, total);
      } finally {
        active -= 1;
      }
    });
    const fetchRange = fetchRangeMock as unknown as typeof fetch;

    const url = "http://asset.localhost/model.ply?previewSession=abc&retry=2";
    const source = createSplatRangeSource(url, total, fetchRange);
    const reader = source.response.body!.getReader();
    const loaded = new Uint8Array(total);
    const progress: number[] = [];
    let received = 0;
    while (true) {
      const { value, done } = await reader.read();
      if (done) break;
      loaded.set(value, received);
      received += value.byteLength;
      progress.push(received / total);
    }

    expect(loaded).toEqual(sourceBytes);
    expect(ranges).toEqual([
      `bytes=0-${SPLAT_RANGE_CHUNK_BYTES - 1}`,
      `bytes=${SPLAT_RANGE_CHUNK_BYTES}-${SPLAT_RANGE_CHUNK_BYTES * 2 - 1}`,
      `bytes=${SPLAT_RANGE_CHUNK_BYTES * 2}-${total - 1}`,
    ]);
    expect(urls).toEqual([url, url, url]);
    expect(maximumActive).toBe(1);
    expect(progress.at(-1)).toBe(1);
    expect(progress.every((value, index) => index === 0 || value > progress[index - 1])).toBe(true);
    expect(source.response.headers.get("content-length")).toBe(String(total));
    expect(fetchRangeMock).toHaveBeenCalledTimes(3);
    for (const [, init] of fetchRangeMock.mock.calls) {
      expect(init?.cache).toBe("no-store");
      expect(new Headers(init?.headers).has("range")).toBe(true);
    }
  });

  it("aborts an in-flight range request idempotently", async () => {
    let capturedSignal: AbortSignal | undefined;
    const fetchRange = vi.fn((_input: string | URL | Request, init?: RequestInit) => {
      capturedSignal = init?.signal ?? undefined;
      return new Promise<Response>((_resolve, reject) => {
        capturedSignal?.addEventListener("abort", () => reject(capturedSignal?.reason), { once: true });
      });
    }) as typeof fetch;
    const source = createSplatRangeSource("http://asset.localhost/large.ply", 32, fetchRange);
    const reading = source.response.arrayBuffer();
    await vi.waitFor(() => expect(fetchRange).toHaveBeenCalledTimes(1));

    source.abort();
    source.abort();

    await expect(reading).rejects.toBeTruthy();
    expect(source.aborted).toBe(true);
    expect(capturedSignal?.aborted).toBe(true);
  });

  it.each([
    {
      name: "a full-file response",
      response: () => partialResponse(new Uint8Array([1, 2, 3]), 0, 3),
      status: 200,
      code: "range-unsupported",
    },
    {
      name: "a missing Content-Range header",
      response: () => new Response(new ArrayBuffer(3), { status: 206 }),
      code: "invalid-content-range",
    },
    {
      name: "a changed total file size",
      response: () => new Response(new ArrayBuffer(3), {
        status: 206,
        headers: { "Content-Range": "bytes 0-2/4" },
      }),
      code: "file-size-changed",
    },
    {
      name: "an unexpected range",
      response: () => new Response(new ArrayBuffer(3), {
        status: 206,
        headers: { "Content-Range": "bytes 1-3/3" },
      }),
      code: "invalid-content-range",
    },
    {
      name: "a short body",
      response: () => new Response(new ArrayBuffer(2), {
        status: 206,
        headers: { "Content-Range": "bytes 0-2/3" },
      }),
      code: "invalid-chunk",
    },
    {
      name: "an empty body",
      response: () => new Response(new ArrayBuffer(0), {
        status: 206,
        headers: { "Content-Range": "bytes 0-2/3" },
      }),
      code: "invalid-chunk",
    },
  ])("rejects $name without issuing a full GET", async ({ response, status, code }) => {
    const fetchRange = vi.fn(async (_input: string | URL | Request, init?: RequestInit) => {
      expect(new Headers(init?.headers).get("range")).toBe("bytes=0-2");
      const result = response();
      if (status === undefined) return result;
      return new Response(await result.arrayBuffer(), { status });
    }) as typeof fetch;
    const source = createSplatRangeSource("http://asset.localhost/large.ply", 3, fetchRange);

    await expect(source.response.arrayBuffer()).rejects.toMatchObject({ code });
    expect(fetchRange).toHaveBeenCalledTimes(1);
  });
});
