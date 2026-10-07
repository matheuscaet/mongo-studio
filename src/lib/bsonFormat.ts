/**
 * How document values read in the result views: type names and tags, the
 * text of a value with its BSON wrapper taken off, and its syntax tone.
 * Everything here works on the relaxed Extended JSON the backend sends.
 */
import {
  bsonLiteral,
  bsonTypeName,
  childEntries,
  dateIso,
  isPlainObject,
  plainText,
  relativeTime,
  uuidText,
} from "./bsonValue";

/**
 * The type name a grid header shows, e.g. "ObjectId" or "Decimal128".
 *
 * Relaxed Extended JSON writes Int32, Int64 and Double alike as plain
 * numbers, so an integer stays "Number"; only a fraction is surely a Double.
 */
export function typeName(value: unknown): string {
  if (typeof value === "number" && !Number.isInteger(value)) return "Double";
  return bsonTypeName(value);
}

const SHORT_TYPES: Record<string, string> = {
  ObjectId: "oid",
  String: "str",
  Number: "num",
  Double: "dbl",
  Int32: "int",
  Int64: "long",
  Decimal128: "dec",
  Date: "date",
  Boolean: "bool",
  Null: "null",
  Object: "obj",
  Document: "obj",
  Binary: "bin",
  UUID: "uuid",
  Regex: "regex",
  Timestamp: "ts",
  Symbol: "sym",
  Undefined: "undef",
};

/** The short type tag beside an inspector value, e.g. "oid" or "arr[3]". */
export function shortTypeName(value: unknown): string {
  if (Array.isArray(value)) return `arr[${value.length}]`;
  const name = typeName(value);
  return SHORT_TYPES[name] ?? name.toLowerCase();
}

/** Syntax color family of a value. */
export type ValueTone = "string" | "number" | "keyword" | "bson" | "null" | "nested";

const NUMERIC_TAGS = new Set(["$numberDecimal", "$numberLong", "$numberInt", "$numberDouble"]);

export function valueTone(value: unknown): ValueTone {
  if (value === null || value === undefined) return "null";
  if (childEntries(value) !== null) return "nested";
  switch (typeof value) {
    case "string":
      return "string";
    case "number":
      return "number";
    case "boolean":
      return "keyword";
  }
  const tag = isPlainObject(value) ? Object.keys(value)[0] : "";
  return NUMERIC_TAGS.has(tag) ? "number" : "bson";
}

/** The string inside a single-key wrapper like `{ $oid: "..." }`, or null. */
function wrapped(value: unknown, tag: string): string | null {
  if (!isPlainObject(value)) return null;
  const keys = Object.keys(value);
  if (keys.length !== 1 || keys[0] !== tag) return null;
  const inner = value[tag];
  return typeof inner === "string" ? inner : null;
}

/** The hex of an ObjectId, or null for anything else. */
export function objectIdHex(value: unknown): string | null {
  return wrapped(value, "$oid");
}

/** A date as "YYYY-MM-DD HH:MM" (UTC), with seconds when asked; null if not a date. */
export function formatDate(value: unknown, withSeconds = false): string | null {
  const iso = dateIso(value);
  if (iso === null) return null;
  const time = new Date(iso).getTime();
  if (!Number.isFinite(time)) return iso;
  const normal = new Date(time).toISOString();
  // six-digit years (+010000-...) don't slice cleanly; show them whole
  if (!/^\d{4}-/.test(normal)) return normal;
  return normal.slice(0, withSeconds ? 19 : 16).replace("T", " ");
}

/** A scalar's text without its BSON wrapper: bare hex or UUID, bare decimal, trimmed date. */
function scalarText(value: unknown, withSeconds: boolean): string {
  if (value === null || value === undefined) return "null";
  if (typeof value === "string") return value;
  if (typeof value === "number" || typeof value === "boolean") return String(value);
  const date = formatDate(value, withSeconds);
  if (date !== null) return date;
  for (const tag of ["$oid", ...NUMERIC_TAGS]) {
    const inner = wrapped(value, tag);
    if (inner !== null) return inner;
  }
  const uuid = uuidText(value);
  if (uuid !== null) return uuid;
  return bsonLiteral(value) ?? plainText(value);
}

export function countLabel(n: number, noun: string): string {
  return `${n} ${noun}${n === 1 ? "" : "s"}`;
}

/** A grid cell's text: dates to the minute, nested values as counts. */
export function cellText(value: unknown): string {
  if (Array.isArray(value)) return `[${countLabel(value.length, "item")}]`;
  const entries = childEntries(value);
  if (entries !== null) return `{${countLabel(entries.length, "field")}}`;
  return scalarText(value, false);
}

/** An inspector value's text: dates to the second, nested values summarized. */
export function inspectorText(value: unknown): string {
  if (Array.isArray(value)) return `[ ${countLabel(value.length, "item")} ]`;
  const entries = childEntries(value);
  if (entries !== null) return `{ ${countLabel(entries.length, "field")} }`;
  return scalarText(value, true);
}

/** A value's full text for a hover title; dates add how long ago they were. */
export function valueTitle(value: unknown): string {
  const iso = dateIso(value);
  if (iso !== null) {
    const relative = relativeTime(iso);
    return relative ? `${iso} (${relative})` : iso;
  }
  return plainText(value);
}

/** Identity of a document across result pages and edits: its `_id`, serialized. */
export function documentKey(doc: unknown): string | null {
  if (!isPlainObject(doc) || !("_id" in doc)) return null;
  return JSON.stringify(doc._id);
}

/** A document's `_id` as text: the bare hex for an ObjectId. */
export function documentIdText(doc: unknown): string | null {
  if (!isPlainObject(doc) || !("_id" in doc)) return null;
  return scalarText(doc._id, true);
}

/** A short human label for a document: its first string field other than `_id`. */
export function documentLabel(doc: unknown): string | null {
  if (!isPlainObject(doc)) return null;
  for (const [key, value] of Object.entries(doc)) {
    if (key !== "_id" && typeof value === "string" && value.trim() !== "") {
      return value.length > 60 ? `${value.slice(0, 60)}…` : value;
    }
  }
  return null;
}

/** Whether a value is a non-empty list of documents, which reads best as a grid. */
export function isDocumentList(value: unknown): value is Record<string, unknown>[] {
  return (
    Array.isArray(value) &&
    value.length > 0 &&
    value.every((item) => isPlainObject(item) && bsonLiteral(item) === null)
  );
}
