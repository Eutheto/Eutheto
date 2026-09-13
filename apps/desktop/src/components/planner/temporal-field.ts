import type { TemporalDraft, TemporalRawDraft } from "./field-contracts";

/** Completeness only: Rust owns time syntax, calendar validity and zone resolution. */
export function parseTemporalDraft(raw: TemporalRawDraft): TemporalDraft {
  if (raw.kind === "localInterval") {
    if (raw.startsAt === "" && raw.endsAt === "") return { raw, status: "empty" };
    if (raw.startsAt === "" || raw.endsAt === "") {
      return { raw, status: "incomplete", error: "endpoints" };
    }
    return {
      raw,
      status: "readyForNativeValidation",
      candidate: { startsAt: raw.startsAt, endsAt: raw.endsAt },
    };
  }

  // Normalize only for bounded numeric conversion; never change the raw offset.
  const digits = raw.endDayOffset.replace(/^0+/, "") || "0";
  const offsetValid =
    raw.endDayOffset !== "" &&
    !/[^0-9]/.test(raw.endDayOffset) &&
    digits.length <= 3 &&
    Number(digits) <= 255;
  if (raw.endDayOffset !== "" && !offsetValid) {
    return { raw, status: "incomplete", error: "dayOffset" };
  }
  if (raw.startTime === "" && raw.endTime === "") return { raw, status: "empty" };
  if (raw.startTime === "" || raw.endTime === "") {
    return { raw, status: "incomplete", error: "endpoints" };
  }
  if (!offsetValid) return { raw, status: "incomplete", error: "dayOffset" };
  return {
    raw,
    status: "readyForNativeValidation",
    candidate: {
      kind: "localWindow",
      startTime: raw.startTime,
      endTime: raw.endTime,
      endDayOffset: Number(digits),
    },
  };
}
