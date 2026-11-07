/**
 * A field's value as text to edit, and back.
 *
 * The value comes from `read_field` as canonical Extended JSON, where every
 * number keeps its BSON type. The text is mongosh-style, and reads back with
 * parseQueryValue to the same BSON: Int32 as a plain number, Int64 as
 * NumberLong("…"), a whole Double as Double(5) (a plain 5 would come back an
 * Int32), binary data in full. Types the shell syntax has no helper for are
 * written as their Extended JSON object, which the backend reads as is.
 *
 * Not jsonText: that one is for reading, and shortens long binary data.
 */
import { isPlainObject, uuidText } from "./bsonValue";
import { parseQueryValue } from "./queryText";

const INDENT = "  ";
const IDENTIFIER = /^[A-Za-z_$][\w$]*$/;

const key = (k: string) => (IDENTIFIER.test(k) ? k : JSON.stringify(k));

function doubleText(text: string): string {
  if (text === "NaN" || text === "Infinity") return text;
  if (text === "-Infinity") return 'Double("-Infinity")';
  const n = Number(text);
  if (Object.is(n, -0)) return 'Double("-0.0")';
  // a whole number would read back as an Int32
  return Number.isInteger(n) ? `Double(${String(n)})` : String(n);
}

function dateText(date: unknown): string | null {
  const ms = isPlainObject(date) && typeof date.$numberLong === "string" ? Number(date.$numberLong) : null;
  if (ms === null || !Number.isSafeInteger(ms)) return null;
  const d = new Date(ms);
  const year = d.getUTCFullYear();
  // ISODate reads four-digit years only
  if (Number.isNaN(d.getTime()) || year < 0 || year > 9999) return null;
  return `ISODate("${d.toISOString()}")`;
}

/** A BSON wrapper as shell text; null when `value` is a plain object. */
function bsonText(value: Record<string, unknown>): string | null {
  const keys = Object.keys(value);
  if (keys.length === 0 || !keys[0].startsWith("$")) return null;
  const tag = keys[0];
  const inner = value[tag];
  switch (tag) {
    case "$oid":
      return typeof inner === "string" ? `ObjectId("${inner}")` : null;
    case "$date":
      return dateText(inner);
    case "$numberInt":
      return typeof inner === "string" ? String(Number(inner)) : null;
    case "$numberLong":
      return typeof inner === "string" ? `NumberLong("${inner}")` : null;
    case "$numberDouble":
      return typeof inner === "string" ? doubleText(inner) : null;
    case "$numberDecimal":
      return typeof inner === "string" ? `Decimal128("${inner}")` : null;
    case "$binary": {
      if (!isPlainObject(inner) || typeof inner.base64 !== "string") return null;
      const uuid = uuidText(value);
      if (uuid !== null) return `UUID("${uuid}")`;
      const sub = parseInt(String(inner.subType ?? "0"), 16);
      return Number.isNaN(sub) ? null : `BinData(${sub}, "${inner.base64}")`;
    }
    case "$timestamp":
      return isPlainObject(inner) ? `Timestamp(${String(inner.t)}, ${String(inner.i)})` : null;
    case "$regularExpression": {
      if (!isPlainObject(inner)) return null;
      const pattern = String(inner.pattern ?? "");
      // a slash or a line break would end or break the literal
      if (/[/\n\r]/.test(pattern) || pattern === "") return null;
      return `/${pattern}/${String(inner.options ?? "")}`;
    }
    case "$minKey":
      return "MinKey";
    case "$maxKey":
      return "MaxKey";
    default:
      return null;
  }
}

function write(value: unknown, depth: number): string {
  if (value === null || value === undefined) return "null";
  switch (typeof value) {
    case "string":
      return JSON.stringify(value);
    case "number":
    case "boolean":
      return String(value);
  }
  if (isPlainObject(value)) {
    const tagged = bsonText(value);
    if (tagged !== null) return tagged;
  }
  const pad = INDENT.repeat(depth + 1);
  const end = INDENT.repeat(depth);
  if (Array.isArray(value)) {
    if (value.length === 0) return "[]";
    return `[\n${value.map((item) => pad + write(item, depth + 1)).join(",\n")}\n${end}]`;
  }
  const entries = Object.entries(value as Record<string, unknown>);
  if (entries.length === 0) return "{}";
  return `{\n${entries.map(([k, v]) => `${pad}${key(k)}: ${write(v, depth + 1)}`).join(",\n")}\n${end}}`;
}

/** Canonical Extended JSON as text to edit. */
export function fieldText(canonical: unknown): string {
  return write(canonical, 0);
}

export type ParsedField = { ok: true; value: unknown } | { ok: false; error: string };

/** Edited text as the Extended JSON to $set; an error says where it's wrong. */
export function parseFieldText(text: string): ParsedField {
  try {
    const value = parseQueryValue(text, "value");
    if (value === undefined) return { ok: false, error: "Type a value, e.g. [] or {}" };
    return { ok: true, value };
  } catch (e) {
    return { ok: false, error: e instanceof Error ? e.message : String(e) };
  }
}

/**
 * Whether `name` can be a field name: $set and $rename would read a dot as
 * a path and a leading $ as an operator. Mirrors the backend's check.
 */
export function fieldNameError(name: string): string | null {
  if (name === "") return "A field needs a name";
  if (name.includes(".")) return "A field name can't contain a dot";
  if (name.startsWith("$")) return "A field name can't start with $";
  return null;
}

/** The value at `path` in a document, following array items by index. */
export function valueAt(doc: unknown, path: string[]): unknown {
  let current: unknown = doc;
  for (const segment of path) {
    if (Array.isArray(current)) current = current[Number(segment)];
    else if (isPlainObject(current)) current = current[segment];
    else return undefined;
  }
  return current;
}

let counter = Math.floor(Math.random() * 0xffffff);
const machine = Array.from({ length: 5 }, () => Math.floor(Math.random() * 256));

/** A new ObjectId as the drivers make one: seconds, 5 random bytes, a counter. */
export function newObjectId(now = Date.now()): string {
  counter = (counter + 1) % 0x1000000;
  const bytes = [
    ...[24, 16, 8, 0].map((shift) => (Math.floor(now / 1000) >>> shift) & 0xff),
    ...machine,
    (counter >> 16) & 0xff,
    (counter >> 8) & 0xff,
    counter & 0xff,
  ];
  return bytes.map((b) => b.toString(16).padStart(2, "0")).join("");
}
