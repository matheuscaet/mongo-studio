/**
 * Helpers for reading the relaxed Extended JSON the backend sends, where BSON
 * types arrive as single-key wrapper objects like `{"$oid": "..."}`.
 */

export function isPlainObject(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

/** The wrapper's single tag, or null when this is a genuine object. */
function bsonTag(value: unknown): string | null {
  if (!isPlainObject(value)) return null;
  const keys = Object.keys(value);
  if (keys.length !== 1 || !keys[0].startsWith("$")) return null;
  return keys[0];
}

/**
 * How binary subtype 3 values (legacy UUIDs) read. Subtype 3 never fixed a
 * byte order, so each old driver wrote the 16 bytes its own way: Python as
 * they are, Java with each 8-byte half reversed, .NET with its first three
 * groups reversed. "raw" shows them as plain binary. Set per connection.
 */
export type LegacyUuidEncoding = "raw" | "python" | "java" | "csharp";

/** The constructor a legacy UUID shows as, which filters read back. */
export const LEGACY_UUID_CONSTRUCTORS: Record<Exclude<LegacyUuidEncoding, "raw">, string> = {
  python: "LegacyPythonUUID",
  java: "LegacyJavaUUID",
  csharp: "LegacyCSharpUUID",
};

/** The hex of a 16-byte binary of this subtype, or null for anything else. */
function binaryHex(value: unknown, subType: string): string | null {
  if (!isPlainObject(value) || Object.keys(value).length !== 1) return null;
  const bin = value.$binary;
  if (!isPlainObject(bin) || bin.subType !== subType || typeof bin.base64 !== "string") return null;
  let bytes: string;
  try {
    bytes = atob(bin.base64);
  } catch {
    return null;
  }
  if (bytes.length !== 16) return null;
  return Array.from(bytes, (c) => c.charCodeAt(0).toString(16).padStart(2, "0")).join("");
}

function dashed(hex: string): string {
  return `${hex.slice(0, 8)}-${hex.slice(8, 12)}-${hex.slice(12, 16)}-${hex.slice(16, 20)}-${hex.slice(20)}`;
}

/** Reverses the bytes of a stretch of hex. */
function reversedBytes(hex: string): string {
  return (hex.match(/../g) ?? []).reverse().join("");
}

/**
 * Moves 32 hex digits between standard UUID order and a driver's legacy
 * order. Each reordering undoes itself, so the same call goes both ways.
 */
export function legacyByteOrder(hex: string, encoding: LegacyUuidEncoding): string {
  switch (encoding) {
    case "java":
      return reversedBytes(hex.slice(0, 16)) + reversedBytes(hex.slice(16));
    case "csharp":
      return reversedBytes(hex.slice(0, 8)) + reversedBytes(hex.slice(8, 12)) + reversedBytes(hex.slice(12, 16)) + hex.slice(16);
    default:
      return hex;
  }
}

/**
 * The text of a UUID (binary subtype 4, 16 bytes), dashed and lowercase, or
 * null for anything else. Relaxed Extended JSON sends a UUID as any other
 * binary, `{ $binary: { base64, subType: "04" } }`, which nobody can read
 * or search by.
 */
export function uuidText(value: unknown): string | null {
  const hex = binaryHex(value, "04");
  return hex === null ? null : dashed(hex);
}

/** A legacy UUID (binary subtype 3, 16 bytes) read in this byte order; null when raw. */
export function legacyUuidText(value: unknown, encoding: LegacyUuidEncoding): string | null {
  if (encoding === "raw") return null;
  const hex = binaryHex(value, "03");
  return hex === null ? null : dashed(legacyByteOrder(hex, encoding));
}

/** Whether a dashed UUID has a known version (1-8) and the RFC variant. */
function hasUuidVersion(text: string): boolean {
  return /^[1-8]$/.test(text[14]) && /^[89ab]$/.test(text[19]);
}

/** What `detectLegacyUuid` found: the byte order and how many values it explains. */
export interface LegacyUuidGuess {
  encoding: Exclude<LegacyUuidEncoding, "raw">;
  matched: number;
  sampled: number;
}

const DETECT_ORDERS = ["java", "csharp", "python"] as const;

/**
 * The byte order the documents' legacy UUIDs were most likely written in.
 * Nearly every UUID has a version digit and the RFC variant, so the order
 * that gives them to nearly all sampled values is the one that wrote them.
 * Null with too few values to tell, or no clear winner.
 */
export function detectLegacyUuid(documents: unknown[], limit = 200): LegacyUuidGuess | null {
  const found: unknown[] = [];
  const visit = (value: unknown, depth: number) => {
    if (found.length >= limit || depth > 8) return;
    if (binaryHex(value, "03") !== null) {
      found.push(value);
    } else if (Array.isArray(value)) {
      for (const item of value) visit(item, depth + 1);
    } else if (isPlainObject(value) && bsonTag(value) === null) {
      for (const item of Object.values(value)) visit(item, depth + 1);
    }
  };
  for (const doc of documents) visit(doc, 0);
  if (found.length < 3) return null;
  const counts = DETECT_ORDERS.map((encoding) => ({
    encoding,
    matched: found.filter((v) => hasUuidVersion(legacyUuidText(v, encoding) as string)).length,
  })).sort((a, b) => b.matched - a.matched);
  const [best, next] = counts;
  // random bytes pass the check about one time in 26: demand a clear lead
  if (best.matched < found.length * 0.9 || next.matched > found.length * 0.5) return null;
  return { encoding: best.encoding, matched: best.matched, sampled: found.length };
}

/**
 * Renders a BSON wrapper as the scalar it stands for, spelled the way mongosh
 * spells it. Returns null for anything that is a genuine object.
 */
export function bsonLiteral(value: unknown, legacy: LegacyUuidEncoding = "raw"): string | null {
  const tag = bsonTag(value);
  if (tag === null || !isPlainObject(value)) return null;

  switch (tag) {
    case "$oid":
      return `ObjectId("${String(value.$oid)}")`;
    case "$date": {
      const date = value.$date;
      if (typeof date === "string") return `ISODate("${date}")`;
      // out-of-range dates stay as { $date: { $numberLong } } even in relaxed mode
      if (isPlainObject(date) && typeof date.$numberLong === "string") {
        return `ISODate(${date.$numberLong})`;
      }
      return null;
    }
    case "$numberDecimal":
      return `Decimal128("${String(value.$numberDecimal)}")`;
    case "$numberLong":
      return `Long("${String(value.$numberLong)}")`;
    case "$numberInt":
      return String(value.$numberInt);
    case "$numberDouble":
      return String(value.$numberDouble);
    case "$timestamp": {
      const ts = value.$timestamp;
      if (!isPlainObject(ts)) return null;
      return `Timestamp(${String(ts.t)}, ${String(ts.i)})`;
    }
    case "$binary": {
      // spelled as a query takes it back: { field: UUID("…") }
      const uuid = uuidText(value);
      if (uuid !== null) return `UUID("${uuid}")`;
      const legacyUuid = legacyUuidText(value, legacy);
      if (legacyUuid !== null && legacy !== "raw") return `${LEGACY_UUID_CONSTRUCTORS[legacy]}("${legacyUuid}")`;
      const bin = value.$binary;
      if (!isPlainObject(bin)) return null;
      const base64 = String(bin.base64 ?? "");
      const shown = base64.length > 24 ? `${base64.slice(0, 24)}…` : base64;
      return `BinData(${String(bin.subType ?? "00")}, "${shown}")`;
    }
    case "$regularExpression": {
      const re = value.$regularExpression;
      if (!isPlainObject(re)) return null;
      return `/${String(re.pattern ?? "")}/${String(re.options ?? "")}`;
    }
    case "$code":
      return `Code(${JSON.stringify(value.$code)})`;
    case "$symbol":
      return `Symbol("${String(value.$symbol)}")`;
    case "$minKey":
      return "MinKey";
    case "$maxKey":
      return "MaxKey";
    case "$undefined":
      return "undefined";
    default:
      return null;
  }
}

/** Children of an object/array node, or null for anything that reads as a scalar. */
export function childEntries(value: unknown): [string, unknown][] | null {
  if (Array.isArray(value)) return value.map((item, i) => [String(i), item]);
  if (isPlainObject(value) && bsonLiteral(value) === null) return Object.entries(value);
  return null;
}

const TAG_TYPES: Record<string, string> = {
  $oid: "ObjectId",
  $date: "Date",
  $numberDecimal: "Decimal128",
  $numberLong: "Int64",
  $numberInt: "Int32",
  $numberDouble: "Double",
  $binary: "Binary",
  $regularExpression: "Regex",
  $timestamp: "Timestamp",
  $minKey: "MinKey",
  $maxKey: "MaxKey",
  $code: "Code",
  $symbol: "Symbol",
  $undefined: "Undefined",
};

/**
 * The BSON type name to show in the Type column.
 *
 * Relaxed Extended JSON writes Int32, Int64 and Double all as plain JSON
 * numbers, so those collapse to a single "Number" here — claiming "Int32"
 * would be a guess. Types that survive as wrappers are named exactly.
 */
export function bsonTypeName(value: unknown, isRoot = false, legacy: LegacyUuidEncoding = "raw"): string {
  if (value === null) return "Null";
  if (Array.isArray(value)) return "Array";
  if (isPlainObject(value)) {
    const tag = bsonTag(value);
    if (uuidText(value) !== null) return "UUID";
    if (legacyUuidText(value, legacy) !== null) return "LegacyUUID";
    if (tag !== null && bsonLiteral(value) !== null) return TAG_TYPES[tag] ?? "Object";
    return isRoot ? "Document" : "Object";
  }
  switch (typeof value) {
    case "string":
      return "String";
    case "number":
      return "Number";
    case "boolean":
      return "Boolean";
    default:
      return "Unknown";
  }
}

/** The ISO string inside a `{ $date }` wrapper, or null. */
export function dateIso(value: unknown): string | null {
  if (!isPlainObject(value)) return null;
  const date = value.$date;
  if (typeof date === "string") return date;
  if (isPlainObject(date) && typeof date.$numberLong === "string") {
    const ms = Number(date.$numberLong);
    // past +-8.64e15 ms a Date is invalid and toISOString() throws
    return Number.isFinite(ms) && Math.abs(ms) <= 8.64e15 ? new Date(ms).toISOString() : null;
  }
  return null;
}

const UNITS: [Intl.RelativeTimeFormatUnit, number][] = [
  ["year", 365 * 24 * 3600_000],
  ["month", 30 * 24 * 3600_000],
  ["day", 24 * 3600_000],
  ["hour", 3600_000],
  ["minute", 60_000],
  ["second", 1000],
];

/** "2 days ago" / "in 3 months", or null when the date can't be read. */
export function relativeTime(iso: string, now = Date.now()): string | null {
  const then = new Date(iso).getTime();
  if (!Number.isFinite(then)) return null;
  const diff = then - now;
  const fmt = new Intl.RelativeTimeFormat(undefined, { numeric: "auto" });
  for (const [unit, ms] of UNITS) {
    if (Math.abs(diff) >= ms) return fmt.format(Math.round(diff / ms), unit);
  }
  return fmt.format(0, "second");
}

/** Plain-text rendering of any value, for a cell's hover title. */
export function plainText(value: unknown, maxLength = 400, legacy: LegacyUuidEncoding = "raw"): string {
  const literal = bsonLiteral(value, legacy);
  const text =
    literal !== null
      ? literal
      : typeof value === "string"
        ? value
        : JSON.stringify(value) ?? String(value);
  return text.length > maxLength ? `${text.slice(0, maxLength)}…` : text;
}
