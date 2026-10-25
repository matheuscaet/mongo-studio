/**
 * Pretty-printed, shell-like text of a value, split into syntax tokens:
 * BSON wrappers are written as their constructors, e.g. ObjectId("...").
 */
import { bsonLiteral, dateIso, isPlainObject, LEGACY_UUID_CONSTRUCTORS, legacyUuidText } from "./bsonValue";
import type { LegacyUuidEncoding } from "./bsonValue";

export type JsonTone = "key" | "string" | "number" | "keyword" | "bson" | "punct" | "plain";

export interface JsonToken {
  text: string;
  tone: JsonTone;
}

const INDENT = "  ";

/** Constructors whose single argument is a quoted string. */
const STRING_CONSTRUCTORS: Record<string, string> = {
  $oid: "ObjectId",
  $numberDecimal: "Decimal128",
  $numberLong: "Long",
};

function push(out: JsonToken[], text: string, tone: JsonTone) {
  const last = out[out.length - 1];
  // merge runs of one tone, so the view renders fewer spans
  if (last && last.tone === tone) last.text += text;
  else out.push({ text, tone });
}

function writeConstructor(out: JsonToken[], name: string, arg: string) {
  push(out, name, "bson");
  push(out, "(", "punct");
  push(out, JSON.stringify(arg), "string");
  push(out, ")", "punct");
}

/** Writes a BSON wrapper; false when `value` is a genuine object. */
function writeBson(value: Record<string, unknown>, out: JsonToken[], legacy: LegacyUuidEncoding): boolean {
  if (bsonLiteral(value) === null) return false;
  const tag = Object.keys(value)[0];
  const inner = value[tag];
  const legacyUuid = legacyUuidText(value, legacy);
  if (legacyUuid !== null && legacy !== "raw") {
    writeConstructor(out, LEGACY_UUID_CONSTRUCTORS[legacy], legacyUuid);
  } else if (tag in STRING_CONSTRUCTORS && typeof inner === "string") {
    writeConstructor(out, STRING_CONSTRUCTORS[tag], inner);
  } else if (tag === "$date" && dateIso(value) !== null) {
    writeConstructor(out, "ISODate", dateIso(value) as string);
  } else if (tag === "$numberInt" || tag === "$numberDouble") {
    push(out, String(inner), "number");
  } else {
    push(out, bsonLiteral(value) as string, "bson");
  }
  return true;
}

function write(value: unknown, depth: number, out: JsonToken[], legacy: LegacyUuidEncoding) {
  if (value === null || value === undefined) {
    push(out, "null", "keyword");
    return;
  }
  switch (typeof value) {
    case "string":
      push(out, JSON.stringify(value), "string");
      return;
    case "number":
      push(out, String(value), "number");
      return;
    case "boolean":
      push(out, String(value), "keyword");
      return;
  }
  if (isPlainObject(value) && writeBson(value, out, legacy)) return;

  const isArray = Array.isArray(value);
  const entries: [string, unknown][] = isArray
    ? (value as unknown[]).map((item, i) => [String(i), item])
    : Object.entries(value as Record<string, unknown>);
  const [open, close] = isArray ? ["[", "]"] : ["{", "}"];
  if (entries.length === 0) {
    push(out, open + close, "punct");
    return;
  }
  const pad = INDENT.repeat(depth + 1);
  push(out, open, "punct");
  entries.forEach(([key, child], i) => {
    push(out, `\n${pad}`, "plain");
    if (!isArray) {
      push(out, JSON.stringify(key), "key");
      push(out, ": ", "punct");
    }
    write(child, depth + 1, out, legacy);
    if (i < entries.length - 1) push(out, ",", "punct");
  });
  push(out, `\n${INDENT.repeat(depth)}`, "plain");
  push(out, close, "punct");
}

export function jsonTokens(value: unknown, legacy: LegacyUuidEncoding = "raw"): JsonToken[] {
  const out: JsonToken[] = [];
  write(value, 0, out, legacy);
  return out;
}
