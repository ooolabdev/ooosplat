const BINARY_UNITS = ["B", "KiB", "MiB", "GiB"];

/** Format a byte count with binary units and at most one decimal. The unit is
 * picked after rounding, so 1,048,575 bytes reads "1 MiB", not "1,024 KiB". */
export function formatBinarySize(bytes: number, locale: string): string {
  let value = bytes;
  let unit = 0;
  while (unit < BINARY_UNITS.length - 1 && Math.round(value * 10) / 10 >= 1024) {
    value /= 1024;
    unit += 1;
  }
  return `${new Intl.NumberFormat(locale, { maximumFractionDigits: 1 }).format(value)} ${BINARY_UNITS[unit]}`;
}
