/**
 * Reads query text - a filter, sort, projection or pipeline - written the
 * way mongosh accepts it, into the Extended JSON value the backend takes
 * (see src-tauri/src/ejson.rs: `Document::try_from` reads canonical and
 * relaxed Extended JSON alike).
 *
 * Strict JSON reads exactly as `JSON.parse` reads it. On top of that:
 *
 * - keys without quotes: `status`, `$gt`, dotted paths like `items.0.qty`
 * - 'single-quoted' strings, trailing commas, `//` and `/* *\/` comments
 * - shell helpers, as their Extended JSON:
 *   ObjectId("…")                       → { $oid }
 *   ISODate("…"), Date("…"), new Date() → { $date } (ISO text, or ms)
 *   NumberLong(…), Long(…)              → { $numberLong }
 *   NumberInt(…), Int32(…)              → { $numberInt }
 *   NumberDecimal("…"), Decimal128("…") → { $numberDecimal }
 *   UUID("…")                           → { $uuid }
 *   LegacyJavaUUID("…"), LegacyCSharpUUID("…"), LegacyPythonUUID("…")
 *                                       → { $binary } subtype 3, in that
 *                                         driver's byte order
 *   Timestamp(t, i), BinData(sub, "…"), MinKey, MaxKey
 * - regex literals `/pat/flags` → { $regularExpression: { pattern, options } }
 * - NaN and Infinity → { $numberDouble }
 *
 * It's a small hand-written parser rather than `eval`: the text is data a
 * user typed (or the Assistant wrote), and nothing in it should ever run.
 */

import { legacyByteOrder, type LegacyUuidEncoding } from "./bsonValue";

/** A query that can't be read, with where in the text it went wrong. */
export class QuerySyntaxError extends Error {
  /** What's wrong, without the position (the message adds it). */
  readonly reason: string;
  /** 0-based offset into the text, and how many characters to underline. */
  readonly offset: number;
  readonly length: number;
  /** 1-based, as editors count them. */
  readonly line: number;
  readonly column: number;

  constructor(reason: string, text: string, offset: number, length: number, label?: string) {
    const { line, column } = lineColumn(text, offset);
    const where = `at line ${line}, column ${column}`;
    super(label ? `Invalid ${label}: ${reason} ${where}` : `${reason} ${where}`);
    this.name = "SyntaxError";
    this.reason = reason;
    this.offset = offset;
    this.length = length;
    this.line = line;
    this.column = column;
  }

  /** The message alone: results panes show `String(error)`. */
  override toString(): string {
    return this.message;
  }
}

function lineColumn(text: string, offset: number): { line: number; column: number } {
  let line = 1;
  let lineStart = 0;
  for (let i = 0; i < offset && i < text.length; i++) {
    if (text[i] === "\n") {
      line++;
      lineStart = i + 1;
    }
  }
  return { line, column: offset - lineStart + 1 };
}

// ---------------------------------------------------------------- tokens

type Token =
  | { kind: "punct"; text: "{" | "}" | "[" | "]" | "(" | ")" | ":" | ","; start: number; end: number }
  | { kind: "string"; value: string; start: number; end: number }
  | { kind: "number"; value: number; start: number; end: number }
  | { kind: "regex"; pattern: string; flags: string; start: number; end: number }
  /** An identifier, possibly dotted (`items.0.qty`) or `$`-prefixed. */
  | { kind: "word"; text: string; start: number; end: number }
  | { kind: "end"; start: number; end: number };

/** Thrown inside the tokenizer and parser; the public entry points label it. */
class Failure {
  constructor(
    readonly reason: string,
    readonly offset: number,
    readonly length = 1,
  ) {}
}

const PUNCT = "{}[]():,";
const WORD_START = /[A-Za-z_$]/;
const WORD_CHAR = /[\w$]/;
/** Regex options MongoDB understands (PCRE): the JS-only g, y and d aren't. */
const REGEX_FLAGS = "imsux";

const SIMPLE_ESCAPES: Record<string, string> = {
  '"': '"',
  "'": "'",
  "\\": "\\",
  "/": "/",
  b: "\b",
  f: "\f",
  n: "\n",
  r: "\r",
  t: "\t",
  v: "\v",
};

/** Splits the text into tokens; comments and whitespace are dropped. */
function tokenize(text: string): Token[] {
  const tokens: Token[] = [];
  let i = 0;

  while (i < text.length) {
    const c = text[i];

    if (/\s/.test(c) || c === "﻿") {
      i++;
      continue;
    }
    if (c === "/" && text[i + 1] === "/") {
      while (i < text.length && text[i] !== "\n") i++;
      continue;
    }
    if (c === "/" && text[i + 1] === "*") {
      const close = text.indexOf("*/", i + 2);
      if (close === -1) throw new Failure("Unterminated comment", i, 2);
      i = close + 2;
      continue;
    }

    const start = i;
    if (PUNCT.includes(c)) {
      tokens.push({ kind: "punct", text: c as "{", start, end: ++i });
    } else if (c === '"' || c === "'") {
      const { value, end } = readString(text, i);
      tokens.push({ kind: "string", value, start, end });
      i = end;
    } else if (c === "/") {
      // There's no division in a query, so a slash that doesn't start a
      // comment can only start a regex literal.
      const token = readRegex(text, i);
      tokens.push(token);
      i = token.end;
    } else if (/[\d.+-]/.test(c)) {
      const { value, end } = readNumber(text, i);
      tokens.push({ kind: "number", value, start, end });
      i = end;
    } else if (WORD_START.test(c)) {
      i++;
      while (i < text.length && (WORD_CHAR.test(text[i]) || (text[i] === "." && WORD_CHAR.test(text[i + 1] ?? "")))) i++;
      tokens.push({ kind: "word", text: text.slice(start, i), start, end: i });
    } else {
      throw new Failure(`Unexpected character '${c}'`, i);
    }
  }

  tokens.push({ kind: "end", start: text.length, end: text.length });
  return tokens;
}

function readString(text: string, start: number): { value: string; end: number } {
  const quote = text[start];
  let value = "";
  let i = start + 1;
  while (i < text.length) {
    const c = text[i];
    if (c === quote) return { value, end: i + 1 };
    if (c === "\n" || c === "\r") break;
    if (c !== "\\") {
      value += c;
      i++;
      continue;
    }

    const e = text[i + 1];
    if (e === undefined) break;
    if (e in SIMPLE_ESCAPES) {
      value += SIMPLE_ESCAPES[e];
      i += 2;
    } else if (e === "0" && !/\d/.test(text[i + 2] ?? "")) {
      value += "\0";
      i += 2;
    } else if (e === "u" && text[i + 2] === "{") {
      const m = /^\{([0-9a-fA-F]{1,6})\}/.exec(text.slice(i + 2));
      const code = m ? parseInt(m[1], 16) : NaN;
      if (!m || code > 0x10ffff) throw new Failure("Invalid \\u{…} escape", i, 2);
      value += String.fromCodePoint(code);
      i += 2 + m[0].length;
    } else if (e === "u" || e === "x") {
      const digits = e === "u" ? 4 : 2;
      const hex = text.slice(i + 2, i + 2 + digits);
      if (!new RegExp(`^[0-9a-fA-F]{${digits}}$`).test(hex)) {
        throw new Failure(`Invalid \\${e} escape: expected ${digits} hex digits`, i, 2);
      }
      value += String.fromCharCode(parseInt(hex, 16));
      i += 2 + digits;
    } else if (e === "\n" || e === "\r") {
      // a backslash at the end of a line continues the string, as in JS
      i += e === "\r" && text[i + 2] === "\n" ? 3 : 2;
    } else {
      // JavaScript would quietly read "\d" as "d", which is never what a
      // regex in a string meant; say so instead.
      throw new Failure(`Unknown escape '\\${e}' in a string (write '\\\\${e}' for a backslash)`, i, 2);
    }
  }
  throw new Failure("Unterminated string", start, i - start);
}

function readRegex(text: string, start: number): Token {
  let i = start + 1;
  let inClass = false;
  for (;;) {
    const c = text[i];
    if (c === undefined || c === "\n" || c === "\r") {
      throw new Failure("Unterminated regular expression", start, i - start);
    }
    if (c === "\\") {
      i += 2;
      continue;
    }
    if (c === "[") inClass = true;
    else if (c === "]") inClass = false;
    else if (c === "/" && !inClass) break;
    i++;
  }
  // the pattern goes to MongoDB as written, escapes and all, as mongosh sends it
  const pattern = text.slice(start + 1, i);
  i++;
  const flagsStart = i;
  while (i < text.length && WORD_CHAR.test(text[i])) i++;
  const flags = text.slice(flagsStart, i);
  for (let f = 0; f < flags.length; f++) {
    const flag = flags[f];
    if (!REGEX_FLAGS.includes(flag)) {
      throw new Failure(`Unsupported regex flag '${flag}' (MongoDB takes i, m, s, u and x)`, flagsStart + f);
    }
    if (flags.indexOf(flag) !== f) throw new Failure(`Repeated regex flag '${flag}'`, flagsStart + f);
  }
  // MongoDB stores options sorted; the backend sorts them too
  const options = [...flags].sort().join("");
  return { kind: "regex", pattern, flags: options, start, end: i };
}

const DECIMAL_NUMBER = /^(?:\d+\.?\d*|\.\d+)(?:[eE][+-]?\d+)?/;
const HEX_NUMBER = /^0[xX][0-9a-fA-F]+/;

function readNumber(text: string, start: number): { value: number; end: number } {
  let i = start;
  let sign = 1;
  if (text[i] === "+" || text[i] === "-") {
    if (text[i] === "-") sign = -1;
    i++;
  }
  const rest = text.slice(i);
  let value: number;
  let length: number;
  const hex = HEX_NUMBER.exec(rest);
  const decimal = DECIMAL_NUMBER.exec(rest);
  if (rest.startsWith("Infinity")) {
    value = Infinity;
    length = "Infinity".length;
  } else if (hex) {
    value = parseInt(hex[0].slice(2), 16);
    length = hex[0].length;
  } else if (decimal) {
    // JSON forbids leading zeros, and in JavaScript 010 is octal: refuse
    // rather than guess.
    if (/^0\d/.test(decimal[0])) throw new Failure("Numbers can't start with 0", start, i - start + decimal[0].length);
    value = Number(decimal[0]);
    length = decimal[0].length;
  } else {
    throw new Failure(`Unexpected character '${text[start]}'`, start);
  }
  const end = i + length;
  if (WORD_CHAR.test(text[end] ?? "")) {
    let to = end;
    while (to < text.length && WORD_CHAR.test(text[to])) to++;
    throw new Failure(`Invalid number '${text.slice(start, to)}'`, start, to - start);
  }
  return { value: sign * value, end };
}

// ---------------------------------------------------------------- parsing

/** Deeper than MongoDB itself allows (100 levels); stops runaway recursion. */
const MAX_DEPTH = 200;

const HELPERS = [
  "ObjectId",
  "ISODate",
  "Date",
  "NumberLong",
  "Long",
  "NumberInt",
  "Int32",
  "NumberDecimal",
  "Decimal128",
  "UUID",
  "LegacyJavaUUID",
  "LegacyCSharpUUID",
  "LegacyPythonUUID",
  "Timestamp",
  "BinData",
  "MinKey",
  "MaxKey",
] as const;
type Helper = (typeof HELPERS)[number];

function isHelper(name: string): name is Helper {
  return (HELPERS as readonly string[]).includes(name);
}

class Parser {
  private pos = 0;
  private depth = 0;

  constructor(private readonly tokens: Token[]) {}

  private peek(): Token {
    return this.tokens[this.pos];
  }

  private next(): Token {
    return this.tokens[this.pos++];
  }

  private isPunct(text: string): boolean {
    const t = this.peek();
    return t.kind === "punct" && t.text === text;
  }

  private fail(reason: string, token = this.peek()): never {
    throw new Failure(reason, token.start, Math.max(1, token.end - token.start));
  }

  /** The whole text: one value, then nothing else. */
  document(): unknown {
    const value = this.value();
    if (this.peek().kind !== "end") this.fail("Unexpected text after the end of the query");
    return value;
  }

  value(): unknown {
    const token = this.peek();
    switch (token.kind) {
      case "string":
        this.pos++;
        return token.value;
      case "number":
        this.pos++;
        return numberValue(token.value);
      case "regex":
        this.pos++;
        return { $regularExpression: { pattern: token.pattern, options: token.flags } };
      case "word":
        return this.word();
      case "punct":
        if (token.text === "{") return this.nested(() => this.object());
        if (token.text === "[") return this.nested(() => this.array());
        break;
    }
    return this.fail("Expected a value");
  }

  private nested<T>(read: () => T): T {
    if (++this.depth > MAX_DEPTH) this.fail("Nested too deeply");
    const value = read();
    this.depth--;
    return value;
  }

  private object(): Record<string, unknown> {
    this.next(); // {
    const object: Record<string, unknown> = {};
    while (!this.isPunct("}")) {
      const key = this.key();
      if (!this.isPunct(":")) this.fail("Expected ':' after the field name");
      this.pos++;
      setOwn(object, key, this.value());
      if (this.isPunct(",")) this.pos++;
      else if (!this.isPunct("}")) this.fail("Expected ',' or '}'");
    }
    this.next(); // }
    return object;
  }

  private key(): string {
    const token = this.peek();
    if (token.kind === "string" || token.kind === "word") {
      this.pos++;
      return token.kind === "string" ? token.value : token.text;
    }
    // { 0: … } is a key in JavaScript too
    if (token.kind === "number" && Number.isInteger(token.value) && token.value >= 0) {
      this.pos++;
      return String(token.value);
    }
    return this.fail("Expected a field name or '}'");
  }

  private array(): unknown[] {
    this.next(); // [
    const array: unknown[] = [];
    while (!this.isPunct("]")) {
      array.push(this.value());
      if (this.isPunct(",")) this.pos++;
      else if (!this.isPunct("]")) this.fail("Expected ',' or ']'");
    }
    this.next(); // ]
    return array;
  }

  /** true/false/null, NaN/Infinity, MinKey/MaxKey, or a helper call. */
  private word(): unknown {
    const token = this.next() as Extract<Token, { kind: "word" }>;
    switch (token.text) {
      case "true":
        return true;
      case "false":
        return false;
      case "null":
        return null;
      case "NaN":
        return { $numberDouble: "NaN" };
      case "Infinity":
        return { $numberDouble: "Infinity" };
      case "undefined":
        this.fail("undefined can't be sent to MongoDB; use null", token);
    }

    let name = token;
    if (token.text === "new") {
      const after = this.next();
      if (after.kind !== "word" || !isHelper(after.text)) {
        this.fail(`Expected a type after 'new', e.g. new Date("2026-01-31")`, after);
      }
      name = after;
    }

    if (!this.isPunct("(")) {
      // mongosh takes the bare MinKey and MaxKey too
      if (name.text === "MinKey") return { $minKey: 1 };
      if (name.text === "MaxKey") return { $maxKey: 1 };
      if (isHelper(name.text)) this.fail(`Expected '(' after ${name.text}`);
      this.fail(`Unexpected name '${name.text}': put text in quotes, e.g. "${name.text}"`, name);
    }
    if (name.text.startsWith("db.")) {
      this.fail("Write the query alone, without db.collection.find(…): the console runs commands", name);
    }
    if (!isHelper(name.text)) {
      this.fail(`Unknown function '${name.text}' (supported: ${HELPERS.join(", ")})`, name);
    }

    const open = this.next(); // (
    const args: { value: unknown; token: Token }[] = [];
    while (!this.isPunct(")")) {
      const at = this.peek();
      args.push({ value: this.value(), token: at });
      if (this.isPunct(",")) this.pos++;
      else if (!this.isPunct(")")) this.fail("Expected ',' or ')'");
    }
    const close = this.next(); // )
    const call: Call = { name: name.text, args, whole: { ...open, start: name.start, end: close.end } };
    return helper(name.text, call);
  }
}

/** Own property even for "__proto__", as JSON.parse does. */
function setOwn(object: Record<string, unknown>, key: string, value: unknown) {
  if (key === "__proto__") {
    Object.defineProperty(object, key, { value, enumerable: true, writable: true, configurable: true });
  } else {
    object[key] = value;
  }
}

/** JSON has no NaN or Infinity; Extended JSON spells them as doubles. */
function numberValue(n: number): unknown {
  if (Number.isFinite(n)) return n;
  return { $numberDouble: Number.isNaN(n) ? "NaN" : n > 0 ? "Infinity" : "-Infinity" };
}

// ---------------------------------------------------------------- helpers

interface Call {
  name: Helper;
  args: { value: unknown; token: Token }[];
  /** The whole call, from its name to ')', for errors about the call itself. */
  whole: Token;
}

function failAt(token: Token, reason: string): never {
  throw new Failure(reason, token.start, Math.max(1, token.end - token.start));
}

/** Requires exactly `count` arguments. */
function arity(call: Call, count: number, example: string) {
  if (call.args.length !== count) {
    failAt(call.args[count]?.token ?? call.whole, `${call.name} takes ${count === 1 ? "one argument" : `${count} arguments`}, e.g. ${example}`);
  }
}

/** The 32 lowercase hex digits of a UUID helper's one argument. */
function uuidHex(call: Call, name: Helper): string {
  arity(call, 1, `${name}("3b241101-e2bb-4255-8caf-4136c566a962")`);
  const [{ value, token }] = call.args;
  const hex = typeof value === "string" ? value.replace(/-/g, "") : "";
  if (!/^[0-9a-fA-F]{32}$/.test(hex)) failAt(token, "A UUID is 32 hex characters, with or without dashes");
  return hex.toLowerCase();
}

const LEGACY_HELPER_ORDERS: Record<string, LegacyUuidEncoding> = {
  LegacyJavaUUID: "java",
  LegacyCSharpUUID: "csharp",
  LegacyPythonUUID: "python",
};

const INT32_MIN = -(2 ** 31);
const INT32_MAX = 2 ** 31 - 1;
const INT64_MIN = -(2n ** 63n);
const INT64_MAX = 2n ** 63n - 1n;
const INTEGER_TEXT = /^[+-]?\d+$/;
const DECIMAL_TEXT = /^[+-]?(?:\d+\.?\d*|\.\d+)(?:[eE][+-]?\d+)?$|^[+-]?(?:Infinity|Inf|NaN)$/i;

function helper(name: Helper, call: Call): unknown {
  switch (name) {
    case "ObjectId": {
      arity(call, 1, 'ObjectId("65f0c3a2e4b0a1b2c3d4e5f6")');
      const [{ value, token }] = call.args;
      if (typeof value !== "string" || !/^[0-9a-fA-F]{24}$/.test(value)) {
        failAt(token, "An ObjectId is 24 hex characters");
      }
      return { $oid: value.toLowerCase() };
    }

    case "ISODate":
    case "Date":
      return dateHelper(call);

    case "NumberLong":
    case "Long": {
      if (call.args.length === 0) return { $numberLong: "0" };
      arity(call, 1, `${name}("9007199254740993")`);
      const [{ value, token }] = call.args;
      let text: string;
      if (typeof value === "number") {
        // past 2^53 the digits were already lost reading the number
        if (!Number.isSafeInteger(value)) {
          failAt(token, `${name} needs a whole number; pass large values as a string, e.g. ${name}("9007199254740993")`);
        }
        text = String(value);
      } else if (typeof value === "string" && INTEGER_TEXT.test(value.trim())) {
        text = value.trim();
      } else {
        return failAt(token, `${name} needs a whole number`);
      }
      const big = BigInt(text);
      if (big < INT64_MIN || big > INT64_MAX) failAt(token, `${name} is out of the 64-bit range`);
      return { $numberLong: big.toString() };
    }

    case "NumberInt":
    case "Int32": {
      if (call.args.length === 0) return { $numberInt: "0" };
      arity(call, 1, `${name}(42)`);
      const [{ value, token }] = call.args;
      const n = typeof value === "string" && INTEGER_TEXT.test(value.trim()) ? Number(value) : value;
      if (typeof n !== "number" || !Number.isInteger(n)) failAt(token, `${name} needs a whole number`);
      if (n < INT32_MIN || n > INT32_MAX) failAt(token, `${name} is out of the 32-bit range`);
      return { $numberInt: String(n) };
    }

    case "NumberDecimal":
    case "Decimal128": {
      arity(call, 1, `${name}("10.25")`);
      const [{ value, token }] = call.args;
      // a string keeps every digit; a number is what JavaScript read
      const text = typeof value === "number" && Number.isFinite(value) ? String(value) : value;
      if (typeof text !== "string" || !DECIMAL_TEXT.test(text.trim())) {
        failAt(token, `${name} needs a decimal number, e.g. ${name}("10.25")`);
      }
      return { $numberDecimal: text.trim() };
    }

    case "UUID": {
      const h = uuidHex(call, name);
      return { $uuid: `${h.slice(0, 8)}-${h.slice(8, 12)}-${h.slice(12, 16)}-${h.slice(16, 20)}-${h.slice(20)}` };
    }

    case "LegacyJavaUUID":
    case "LegacyCSharpUUID":
    case "LegacyPythonUUID": {
      // the bytes as that driver stored them, so a value copied from a
      // result finds its document
      const stored = legacyByteOrder(uuidHex(call, name), LEGACY_HELPER_ORDERS[name]);
      const bytes = String.fromCharCode(...(stored.match(/../g) ?? []).map((b) => parseInt(b, 16)));
      return { $binary: { base64: btoa(bytes), subType: "03" } };
    }

    case "Timestamp": {
      arity(call, 2, "Timestamp(1700000000, 1)");
      const [t, i] = call.args;
      for (const part of [t, i]) {
        if (typeof part.value !== "number" || !Number.isInteger(part.value) || part.value < 0 || part.value > 0xffffffff) {
          failAt(part.token, "Timestamp takes two whole numbers: seconds and an increment");
        }
      }
      return { $timestamp: { t: t.value, i: i.value } };
    }

    case "BinData": {
      arity(call, 2, 'BinData(0, "SGVsbG8=")');
      const [sub, data] = call.args;
      if (typeof sub.value !== "number" || !Number.isInteger(sub.value) || sub.value < 0 || sub.value > 255) {
        failAt(sub.token, "BinData's subtype is a number from 0 to 255");
      }
      if (typeof data.value !== "string" || !/^[A-Za-z0-9+/]*={0,2}$/.test(data.value) || data.value.length % 4 !== 0) {
        failAt(data.token, "BinData's data is base64 text");
      }
      return { $binary: { base64: data.value, subType: sub.value.toString(16).padStart(2, "0") } };
    }

    case "MinKey":
    case "MaxKey":
      arity(call, 0, `${name}()`);
      return name === "MinKey" ? { $minKey: 1 } : { $maxKey: 1 };
  }
}

/**
 * ISO 8601 as mongosh's ISODate reads it: a date, optionally a time, and
 * an offset (UTC when there's none - never the machine's time zone).
 */
const ISO_DATE =
  /^(\d{4})-(\d{2})-(\d{2})(?:[Tt ](\d{2}):(\d{2})(?::(\d{2})(?:[.,](\d{1,9}))?)?(Z|z|[+-]\d{2}(?::?\d{2})?)?)?$/;

function dateHelper(call: Call): unknown {
  // ISODate() and new Date() mean now
  if (call.args.length === 0) return dateValue(Date.now());
  arity(call, 1, `${call.name}("2026-01-31T00:00:00Z")`);
  const [{ value, token }] = call.args;
  if (typeof value === "number") {
    if (!Number.isInteger(value)) failAt(token, "A date in milliseconds is a whole number");
    return dateValue(value);
  }
  const ms = typeof value === "string" ? parseIsoDate(value.trim()) : null;
  if (ms === null) failAt(token, "Not a valid ISO date, e.g. \"2026-01-31\" or \"2026-01-31T14:30:00Z\"");
  return dateValue(ms);
}

function parseIsoDate(text: string): number | null {
  const m = ISO_DATE.exec(text);
  if (!m) return null;
  const [, y, mo, d, h = "0", mi = "0", s = "0", frac = "", zone = "Z"] = m;
  const year = Number(y);
  const month = Number(mo);
  const day = Number(d);
  if (month < 1 || month > 12 || day < 1 || day > daysIn(year, month)) return null;
  if (Number(h) > 23 || Number(mi) > 59 || Number(s) > 59) return null;

  // setUTCFullYear, unlike Date.UTC, doesn't read years 0-99 as 19xx
  const date = new Date(0);
  date.setUTCFullYear(year, month - 1, day);
  date.setUTCHours(Number(h), Number(mi), Number(s), Number(frac.padEnd(3, "0").slice(0, 3)));
  let ms = date.getTime();
  if (zone !== "Z" && zone !== "z") {
    const sign = zone[0] === "-" ? -1 : 1;
    const digits = zone.slice(1).replace(":", "");
    const offsetH = Number(digits.slice(0, 2));
    const offsetM = Number(digits.slice(2) || "0");
    if (offsetH > 23 || offsetM > 59) return null;
    ms -= sign * (offsetH * 60 + offsetM) * 60_000;
  }
  return ms;
}

function daysIn(year: number, month: number): number {
  if (month === 2) return (year % 4 === 0 && year % 100 !== 0) || year % 400 === 0 ? 29 : 28;
  return [4, 6, 9, 11].includes(month) ? 30 : 31;
}

/**
 * Relaxed Extended JSON's date: ISO text for years 1970-9999, which the
 * backend reads as RFC 3339, and milliseconds outside them.
 */
function dateValue(ms: number): unknown {
  const date = new Date(ms);
  if (Number.isNaN(date.getTime())) return { $date: { $numberLong: String(ms) } };
  const year = date.getUTCFullYear();
  if (year >= 1970 && year <= 9999) return { $date: date.toISOString() };
  return { $date: { $numberLong: String(ms) } };
}

// ---------------------------------------------------------------- entry

function run<T>(text: string, label: string | undefined, read: (tokens: Token[]) => T): T {
  try {
    return read(tokenize(text));
  } catch (e) {
    if (e instanceof Failure) throw new QuerySyntaxError(e.reason, text, e.offset, e.length, label);
    throw e;
  }
}

/** Whether the text holds no query at all: only whitespace and comments. */
function isBlank(tokens: Token[]): boolean {
  return tokens.length === 1;
}

/**
 * Any value written as a query (an object, an array, a string…), or
 * `undefined` when the text is empty or only comments. Throws
 * `QuerySyntaxError`; `label` ("filter", "sort"…) names the field in the
 * message.
 */
export function parseQueryValue(text: string, label?: string): unknown {
  return run(text, label, (tokens) => (isBlank(tokens) ? undefined : new Parser(tokens).document()));
}

/**
 * A filter, sort or projection: an object, or `{}` when the text is empty.
 * Throws `QuerySyntaxError`.
 */
export function parseQueryObject(text: string, label = "filter"): Record<string, unknown> {
  return parseOptionalQueryObject(text, label) ?? {};
}

/** Like `parseQueryObject`, but null when the text is empty: a sort that isn't set. */
export function parseOptionalQueryObject(text: string, label = "sort"): Record<string, unknown> | null {
  const value = parseQueryValue(text, label);
  if (value === undefined) return null;
  if (typeof value !== "object" || value === null || Array.isArray(value)) {
    const example = label === "sort" ? "{ createdAt: -1 }" : '{ status: "paid" }';
    throw new QuerySyntaxError(
      `The ${label} must be an object, e.g. ${example}`,
      text,
      firstTokenOffset(text),
      Math.max(1, text.trim().length),
      undefined,
    );
  }
  return value as Record<string, unknown>;
}

/** A pipeline: an array of stages, or `[]` when the text is empty. Throws `QuerySyntaxError`. */
export function parseQueryArray(text: string, label = "pipeline"): unknown[] {
  const value = parseQueryValue(text, label);
  if (value === undefined) return [];
  if (!Array.isArray(value)) {
    throw new QuerySyntaxError(
      `The ${label} must be an array of stages, e.g. [{ $match: { status: "paid" } }]`,
      text,
      firstTokenOffset(text),
      Math.max(1, text.trim().length),
      undefined,
    );
  }
  return value;
}

function firstTokenOffset(text: string): number {
  return tokenize(text)[0]?.start ?? 0;
}

/**
 * The text on one line with its comments dropped, tokens kept exactly as
 * written: `{ name: 'x', at: ISODate("…") }`. For folding pasted or
 * proposed queries into a one-line field. Null when the text can't be
 * tokenized.
 */
export function compactQueryText(text: string): string | null {
  let tokens: Token[];
  try {
    tokens = tokenize(text);
  } catch {
    return null;
  }
  let out = "";
  let previous: Token | null = null;
  for (const token of tokens) {
    if (token.kind === "end") break;
    const source = text.slice(token.start, token.end);
    if (previous) out += gap(previous, token);
    out += source;
    previous = token;
  }
  return out;
}

/** The space between two tokens on one line, as `inline` in assistant/format.ts writes it. */
function gap(a: Token, b: Token): string {
  const p = (t: Token, s: string) => t.kind === "punct" && t.text === s;
  if (p(a, "{")) return p(b, "}") ? "" : " ";
  if (p(b, "}")) return " ";
  if (p(a, ",") || p(a, ":")) return " ";
  if (p(a, "[") || p(a, "(") || p(b, "]") || p(b, ")") || p(b, ",") || p(b, ":") || p(b, "(")) return "";
  return " ";
}
