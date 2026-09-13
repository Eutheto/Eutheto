import type { DurationDraft, DurationUnit } from "./field-contracts";

const maximumMinutes = 4294967295n;

/** Editable decimal text, not a display-duration or floating-point hours parser. */
export function parseDurationDraft(raw: string, unit: DurationUnit, minimum: 0 | 1): DurationDraft {
  if (raw === "") return { raw, unit, status: "empty" };
  // Use an absolute end assertion: JavaScript's `$` also admits a final newline.
  if (!/^(?:[0-9]+(?:\.[0-9]*)?|\.[0-9]+)(?![\s\S])/.test(raw)) {
    return { raw, unit, status: "invalid", error: "syntax" };
  }

  const dot = raw.indexOf(".");
  const whole = (dot === -1 ? raw : raw.slice(0, dot)).replace(/^0+/, "") || "0";
  let fractionEnd = raw.length;
  if (dot !== -1) {
    while (fractionEnd > dot + 1 && raw[fractionEnd - 1] === "0") fractionEnd -= 1;
  }
  const fraction = dot === -1 ? "" : raw.slice(dot + 1, fractionEnd);
  // Bound all BigInt inputs and powers, including arbitrarily long pasted text.
  // After trailing zero removal, whole-minute hours need at most two decimals.
  if (whole.length > 10) return { raw, unit, status: "invalid", error: "range" };
  if (fraction.length > (unit === "hours" ? 2 : 0)) {
    return { raw, unit, status: "invalid", error: "wholeMinutes" };
  }
  const numerator = BigInt(whole + fraction) * (unit === "hours" ? 60n : 1n);
  const denominator = 10n ** BigInt(fraction.length);
  if (numerator % denominator !== 0n) {
    return { raw, unit, status: "invalid", error: "wholeMinutes" };
  }
  const minutes = numerator / denominator;
  if (minutes < BigInt(minimum) || minutes > maximumMinutes) {
    return { raw, unit, status: "invalid", error: "range" };
  }
  return { raw, unit, status: "valid", minutes: Number(minutes) };
}

/** Null means decimal hours would require repeating digits; never round them. */
export function formatDurationQuantity(minutes: number, unit: DurationUnit): string | null {
  if (unit === "minutes") return String(minutes);
  const quantity = BigInt(minutes);
  if (quantity % 3n !== 0n) return null;
  const hundredths = (quantity * 5n) / 3n;
  const whole = hundredths / 100n;
  const fraction = String(hundredths % 100n)
    .padStart(2, "0")
    .replace(/0+$/, "");
  return fraction ? `${whole.toString()}.${fraction}` : String(whole);
}
