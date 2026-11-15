import { describe, expect, it } from "vitest";
import { formatBinarySize } from "./fileSize";

describe("formatBinarySize", () => {
  it("keeps the familiar unit for ordinary sizes", () => {
    expect(formatBinarySize(0, "en-US")).toBe("0 B");
    expect(formatBinarySize(1023, "en-US")).toBe("1,023 B");
    expect(formatBinarySize(1536, "en-US")).toBe("1.5 KiB");
    expect(formatBinarySize(5 * 1024 ** 3, "en-US")).toBe("5 GiB");
  });

  it("moves to the next unit when rounding reaches 1024", () => {
    expect(formatBinarySize(1024 ** 2 - 1, "en-US")).toBe("1 MiB");
    expect(formatBinarySize(1024 ** 3 - 1, "en-US")).toBe("1 GiB");
    expect(formatBinarySize(1024 * 1023.9, "en-US")).toBe("1,023.9 KiB");
  });

  it("stays in GiB above the largest unit", () => {
    expect(formatBinarySize(2048 * 1024 ** 3, "en-US")).toBe("2,048 GiB");
  });
});
