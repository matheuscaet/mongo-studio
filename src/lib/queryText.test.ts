import { describe, expect, it } from "vitest";
import {
  compactQueryText,
  parseOptionalQueryObject,
  parseQueryArray,
  parseQueryObject,
  parseQueryValue,
  QuerySyntaxError,
} from "./queryText";

/** The error a text raises, for asserting on its message and position. */
function errorOf(read: () => unknown): QuerySyntaxError {
  try {
    read();
  } catch (e) {
    if (e instanceof QuerySyntaxError) return e;
    throw e;
  }
  throw new Error("expected a QuerySyntaxError");
}

const parse = (text: string) => parseQueryValue(text);

describe("strict JSON", () => {
  const samples = [
    "{}",
    "[]",
    '{"a": 1}',
    '{"a": {"$gt": 1.5e3}, "b": [1, -2, 3.25, true, false, null]}',
    '{"_id": {"$oid": "65f0c3a2e4b0a1b2c3d4e5f6"}}',
    '{"s": "quote \\" backslash \\\\ slash \\/ tab \\t nl \\n \\u00e9 \\ud83d\\ude00"}',
    '[{"$match": {"status": "paid"}}, {"$group": {"_id": "$status", "n": {"$sum": 1}}}]',
    '  \n {"nested": {"deep": {"er": [[[]]]}}}  \n',
    '{"a": 0, "b": -0.5, "c": 1E-7, "d": 12345678901234}',
    '"just a string"',
    "42",
    '{"dup": 1, "dup": 2}',
    '{"__proto__": {"x": 1}}',
  ];
  for (const text of samples) {
    it(`reads ${JSON.stringify(text)} like JSON.parse`, () => {
      expect(parse(text)).toStrictEqual(JSON.parse(text));
    });
  }

  it("keeps __proto__ as an own key, not the prototype", () => {
    const value = parse('{"__proto__": {"polluted": true}}') as Record<string, unknown>;
    expect(Object.keys(value)).toEqual(["__proto__"]);
    expect(({} as Record<string, unknown>).polluted).toBeUndefined();
    expect(Object.getPrototypeOf(value)).toBe(Object.prototype);
  });

  it("keeps key order", () => {
    expect(Object.keys(parse("{ b: 1, a: 2, c: 3 }") as object)).toEqual(["b", "a", "c"]);
  });
});

describe("relaxed syntax", () => {
  it("reads the filter from the bug report", () => {
    expect(parse('{ category: "desk" }')).toEqual({ category: "desk" });
  });

  it("takes unquoted keys, $operators and dotted paths", () => {
    expect(parse("{ status: 'paid', total: { $gt: 100 } }")).toEqual({ status: "paid", total: { $gt: 100 } });
    expect(parse("{ $or: [{ a: 1 }, { _b$: 2 }] }")).toEqual({ $or: [{ a: 1 }, { _b$: 2 }] });
    expect(parse("{ a.b: 1, 'c.d': 2, \"e.f\": 3, items.0.qty: { $gte: 5 } }")).toEqual({
      "a.b": 1,
      "c.d": 2,
      "e.f": 3,
      "items.0.qty": { $gte: 5 },
    });
  });

  it("takes keywords and helper names as keys", () => {
    expect(parse("{ true: 1, null: 2, new: 3, Date: 4, ObjectId: 5 }")).toEqual({
      true: 1,
      null: 2,
      new: 3,
      Date: 4,
      ObjectId: 5,
    });
  });

  it("takes integer keys", () => {
    expect(parse("{ 0: 'a', 12: 'b' }")).toEqual({ "0": "a", "12": "b" });
  });

  it("reads nested objects and arrays", () => {
    expect(parse("[{ $match: { tags: { $in: ['a', \"b\"] } } }, { $sort: { at: -1 } }]")).toEqual([
      { $match: { tags: { $in: ["a", "b"] } } },
      { $sort: { at: -1 } },
    ]);
  });

  it("reads single-quoted strings and their escapes", () => {
    expect(parse("'it\\'s'")).toBe("it's");
    expect(parse("'say \"hi\"'")).toBe('say "hi"');
    expect(parse('"it\'s"')).toBe("it's");
    expect(parse("'\\x41\\u0042\\u{1F600}\\0'")).toBe("AB\u{1F600}\0");
    expect(parse("'a\\\nb'")).toBe("ab");
  });

  it("allows trailing commas", () => {
    expect(parse("{ a: 1, b: [1, 2,], }")).toEqual({ a: 1, b: [1, 2] });
    expect(parse("[{ $limit: 5 },]")).toEqual([{ $limit: 5 }]);
  });

  it("skips comments", () => {
    const text = `// active customers
      {
        status: 'active', /* not "pending" */
        // age: { $gt: 18 },
        tier: "gold" // for now
      }`;
    expect(parse(text)).toEqual({ status: "active", tier: "gold" });
  });

  it("reads numbers in their JavaScript forms", () => {
    expect(parse("[1, -1, +2, 1.5, -0.25, .5, 5., 1e3, -2.5E-2, 0x1F, -0x10]")).toEqual([
      1, -1, 2, 1.5, -0.25, 0.5, 5, 1000, -0.025, 31, -16,
    ]);
  });

  it("spells NaN and Infinity as Extended JSON doubles", () => {
    expect(parse("[NaN, Infinity, -Infinity, +Infinity, 1e999]")).toEqual([
      { $numberDouble: "NaN" },
      { $numberDouble: "Infinity" },
      { $numberDouble: "-Infinity" },
      { $numberDouble: "Infinity" },
      { $numberDouble: "Infinity" },
    ]);
  });

  it("reads strings with slashes and comment markers as text", () => {
    expect(parse("{ url: 'http://x.com/*a*/' }")).toEqual({ url: "http://x.com/*a*/" });
  });
});

describe("shell helpers", () => {
  const oid = "65f0c3a2e4b0a1b2c3d4e5f6";

  it("ObjectId", () => {
    expect(parse(`{ _id: ObjectId("${oid}") }`)).toEqual({ _id: { $oid: oid } });
    expect(parse(`ObjectId('${oid.toUpperCase()}')`)).toEqual({ $oid: oid });
    expect(parse(`new ObjectId("${oid}")`)).toEqual({ $oid: oid });
    expect(parse(`ObjectId ( "${oid}" , )`)).toEqual({ $oid: oid });
  });

  it("legacy UUIDs, as the driver that wrote them stored the bytes", () => {
    const stored = { $binary: { base64: "N02eJg4nE2U4tOZS5E2nsg==", subType: "03" } };
    expect(parse('LegacyJavaUUID("6513270e-269e-4d37-b2a7-4de452e6b438")')).toEqual(stored);
    expect(parse('LegacyCSharpUUID("269e4d37270e651338b4e652e44da7b2")')).toEqual(stored);
    expect(parse('LegacyPythonUUID("374d9e26-0e27-1365-38b4-e652e44da7b2")')).toEqual(stored);
    expect(errorOf(() => parse('LegacyJavaUUID("nope")')).reason).toMatch(/32 hex characters/);
  });

  it("ISODate, Date and new Date", () => {
    expect(parse('ISODate("2026-01-31T14:30:00Z")')).toEqual({ $date: "2026-01-31T14:30:00.000Z" });
    expect(parse('ISODate("2026-01-31")')).toEqual({ $date: "2026-01-31T00:00:00.000Z" });
    expect(parse('new Date("2026-01-31T14:30:00.123456+02:00")')).toEqual({ $date: "2026-01-31T12:30:00.123Z" });
    expect(parse('Date("2026-01-31 14:30")')).toEqual({ $date: "2026-01-31T14:30:00.000Z" });
    expect(parse('ISODate("2024-02-29T00:00:00-0300")')).toEqual({ $date: "2024-02-29T03:00:00.000Z" });
    // milliseconds, as the results show out-of-range dates
    expect(parse("ISODate(0)")).toEqual({ $date: "1970-01-01T00:00:00.000Z" });
    expect(parse('ISODate("1969-12-31T23:59:59Z")')).toEqual({ $date: { $numberLong: "-1000" } });
    expect(parse("ISODate(-62135596800000)")).toEqual({ $date: { $numberLong: "-62135596800000" } });
    expect(parse('ISODate("0050-06-01")')).toEqual({
      $date: { $numberLong: String(Date.parse("0050-06-01T00:00:00Z")) },
    });
  });

  it("ISODate() and new Date() are now", () => {
    const before = Date.now();
    const value = parse("{ at: { $lt: new Date() } }") as { at: { $lt: { $date: string } } };
    const at = Date.parse(value.at.$lt.$date);
    expect(at).toBeGreaterThanOrEqual(before - 1);
    expect(at).toBeLessThanOrEqual(Date.now() + 1);
    expect(parse("ISODate()")).toHaveProperty("$date");
  });

  it("rejects dates that don't exist or aren't ISO", () => {
    for (const bad of ['"2026-02-29"', '"2026-13-01"', '"2026-04-31"', '"2026-01-01T24:00:00Z"', '"Jan 1 2026"', '"2026-1-1"', "1.5", "true"]) {
      expect(errorOf(() => parse(`ISODate(${bad})`)).reason, bad).toMatch(/ISO date|whole number/);
    }
  });

  it("NumberLong and Long", () => {
    expect(parse("NumberLong(42)")).toEqual({ $numberLong: "42" });
    expect(parse('NumberLong("9007199254740993")')).toEqual({ $numberLong: "9007199254740993" });
    expect(parse("Long('-9223372036854775808')")).toEqual({ $numberLong: "-9223372036854775808" });
    expect(parse("new NumberLong(-7)")).toEqual({ $numberLong: "-7" });
    expect(parse("NumberLong()")).toEqual({ $numberLong: "0" });
    expect(errorOf(() => parse("NumberLong(9007199254740993)")).reason).toMatch(/as a string/);
    expect(errorOf(() => parse("NumberLong('9223372036854775808')")).reason).toMatch(/64-bit/);
    expect(errorOf(() => parse("NumberLong(1.5)")).reason).toMatch(/whole number/);
    expect(errorOf(() => parse("NumberLong('12a')")).reason).toMatch(/whole number/);
  });

  it("NumberInt and Int32", () => {
    expect(parse("NumberInt(5)")).toEqual({ $numberInt: "5" });
    expect(parse("Int32('-12')")).toEqual({ $numberInt: "-12" });
    expect(errorOf(() => parse("NumberInt(2147483648)")).reason).toMatch(/32-bit/);
    expect(errorOf(() => parse("NumberInt(1.5)")).reason).toMatch(/whole number/);
  });

  it("NumberDecimal and Decimal128", () => {
    expect(parse('NumberDecimal("10.25")')).toEqual({ $numberDecimal: "10.25" });
    expect(parse("Decimal128('-1.5E+3')")).toEqual({ $numberDecimal: "-1.5E+3" });
    expect(parse("NumberDecimal(3.5)")).toEqual({ $numberDecimal: "3.5" });
    expect(errorOf(() => parse('NumberDecimal("abc")')).reason).toMatch(/decimal number/);
  });

  it("UUID", () => {
    const uuid = "3b241101-e2bb-4255-8caf-4136c566a962";
    expect(parse(`UUID("${uuid}")`)).toEqual({ $uuid: uuid });
    expect(parse(`UUID('${uuid.replace(/-/g, "").toUpperCase()}')`)).toEqual({ $uuid: uuid });
    expect(errorOf(() => parse('UUID("1234")')).reason).toMatch(/32 hex/);
  });

  it("Timestamp, BinData, MinKey and MaxKey", () => {
    expect(parse("Timestamp(1700000000, 3)")).toEqual({ $timestamp: { t: 1700000000, i: 3 } });
    expect(parse('BinData(4, "AAECAwQFBgcICQoLDA0ODw==")')).toEqual({
      $binary: { base64: "AAECAwQFBgcICQoLDA0ODw==", subType: "04" },
    });
    expect(parse("[MinKey, MaxKey(), MinKey()]")).toEqual([{ $minKey: 1 }, { $maxKey: 1 }, { $minKey: 1 }]);
    expect(errorOf(() => parse("Timestamp(1)")).reason).toMatch(/takes 2 arguments/);
  });

  it("checks the ObjectId", () => {
    const e = errorOf(() => parse('{ _id: ObjectId("123") }'));
    expect(e.reason).toBe("An ObjectId is 24 hex characters");
    expect(e.column).toBe(17);
    expect(errorOf(() => parse("ObjectId()")).reason).toMatch(/takes one argument/);
    expect(errorOf(() => parse(`ObjectId("${oid}", 1)`)).reason).toMatch(/takes one argument/);
  });

  it("rejects unknown functions and bare names", () => {
    const unknown = errorOf(() => parse("{ a: Foo(1) }"));
    expect(unknown.reason).toMatch(/^Unknown function 'Foo'/);
    expect(unknown.column).toBe(6);
    expect(errorOf(() => parse("{ a: b }")).reason).toMatch(/^Unexpected name 'b'/);
    expect(errorOf(() => parse("{ a: undefined }")).reason).toMatch(/use null/);
    expect(errorOf(() => parse("new Foo('x')")).reason).toMatch(/after 'new'/);
    expect(errorOf(() => parse("{ a: ObjectId }")).reason).toBe("Expected '(' after ObjectId");
  });
});

describe("regex literals", () => {
  it("become $regularExpression with sorted options", () => {
    expect(parse("{ name: /^ac/i }")).toEqual({ name: { $regularExpression: { pattern: "^ac", options: "i" } } });
    expect(parse("/a/xsmi")).toEqual({ $regularExpression: { pattern: "a", options: "imsx" } });
    expect(parse("{ $regex: /x/ }")).toEqual({ $regex: { $regularExpression: { pattern: "x", options: "" } } });
  });

  it("keep the pattern as written, escapes and classes included", () => {
    expect(parse(String.raw`/a\/b[/\]]c\d+/`)).toEqual({
      $regularExpression: { pattern: String.raw`a\/b[/\]]c\d+`, options: "" },
    });
    expect(parse(String.raw`[/"'/, /'/]`)).toEqual([
      { $regularExpression: { pattern: `"'`, options: "" } },
      { $regularExpression: { pattern: "'", options: "" } },
    ]);
  });

  it("reject flags MongoDB doesn't have", () => {
    const e = errorOf(() => parse("{ a: /x/g }"));
    expect(e.reason).toMatch(/Unsupported regex flag 'g'/);
    expect(e.column).toBe(9);
    expect(errorOf(() => parse("/x/ii")).reason).toMatch(/Repeated/);
  });

  it("must end on their line", () => {
    expect(errorOf(() => parse("{ a: /abc\n/ }")).reason).toBe("Unterminated regular expression");
  });
});

describe("errors", () => {
  it("say what was expected, and where", () => {
    const e = errorOf(() => parse('{ category: "desk" x }'));
    expect(e.message).toBe("Expected ',' or '}' at line 1, column 20");
    expect(e.line).toBe(1);
    expect(e.column).toBe(20);
    expect(e.offset).toBe(19);
    expect(String(e)).toBe(e.message);
  });

  it("count lines and columns", () => {
    const e = errorOf(() => parse("{\n  a: 1,\n  b: [1 2]\n}"));
    expect(e.message).toBe("Expected ',' or ']' at line 3, column 9");
  });

  it("name the field when given a label", () => {
    const e = errorOf(() => parseQueryObject("{ a: }", "filter"));
    expect(e.message).toBe("Invalid filter: Expected a value at line 1, column 6");
    expect(e.reason).toBe("Expected a value");
  });

  const garbage: [string, RegExp][] = [
    ["{ a: }", /^Expected a value/],
    ["{ a: 1 } extra", /^Unexpected text after the end of the query/],
    ["{ a 1 }", /^Expected ':' after the field name/],
    ["{ a: 1", /^Expected ',' or '}'/],
    ["[1, 2", /^Expected ',' or '\]'/],
    ["[1,,2]", /^Expected a value/],
    ["{ , }", /^Expected a field name or '}'/],
    ["{ -1: 2 }", /^Expected a field name or '}'/],
    ["{ a: 'open }", /^Unterminated string/],
    ['{ a: "x\ny" }', /^Unterminated string/],
    ["{ a: 1 /* open", /^Unterminated comment/],
    ['{ a: "\\d" }', /^Unknown escape '\\d'/],
    ['{ a: "\\u12" }', /^Invalid \\u escape/],
    ["{ a: 012 }", /^Numbers can't start with 0/],
    ["{ a: 12abc }", /^Invalid number '12abc'/],
    ["{ a: - }", /^Unexpected character '-'/],
    ["{ a: @ }", /^Unexpected character '@'/],
    ["{ a: `x` }", /^Unexpected character '`'/],
    ["{ a: () }", /^Expected a value/],
    ["{ a: 1 };", /^Unexpected character ';'/],
    ["db.orders.find({})", /^Write the query alone/],
  ];
  for (const [text, reason] of garbage) {
    it(`rejects ${JSON.stringify(text)}`, () => {
      expect(errorOf(() => parse(text)).reason).toMatch(reason);
    });
  }

  it("give up on runaway nesting instead of overflowing the stack", () => {
    expect(errorOf(() => parse("[".repeat(10_000))).reason).toBe("Nested too deeply");
  });

  it("point at the end when the text stops short", () => {
    const e = errorOf(() => parse("{ a: 1"));
    expect(e.offset).toBe(6);
    expect(e.column).toBe(7);
  });
});

describe("parseQueryObject / parseOptionalQueryObject / parseQueryArray", () => {
  it("read empty or comment-only text as the default", () => {
    expect(parseQueryObject("")).toEqual({});
    expect(parseQueryObject("  // nothing yet\n")).toEqual({});
    expect(parseOptionalQueryObject("   ")).toBeNull();
    expect(parseOptionalQueryObject("/* off */")).toBeNull();
    expect(parseQueryArray("")).toEqual([]);
  });

  it("require the right shape", () => {
    expect(errorOf(() => parseQueryObject("[1]", "filter")).message).toBe(
      'The filter must be an object, e.g. { status: "paid" } at line 1, column 1',
    );
    expect(errorOf(() => parseOptionalQueryObject("  'x'", "sort")).message).toMatch(
      /^The sort must be an object, e.g. \{ createdAt: -1 \} at line 1, column 3$/,
    );
    expect(errorOf(() => parseQueryArray("{ $match: {} }")).reason).toMatch(/^The pipeline must be an array of stages/);
  });

  it("read sorts", () => {
    expect(parseOptionalQueryObject("{ createdAt: -1, _id: 1 }")).toEqual({ createdAt: -1, _id: 1 });
  });
});

describe("compactQueryText", () => {
  it("puts the query on one line, tokens as written", () => {
    const text = `{
      // recent
      _id: ObjectId('65f0c3a2e4b0a1b2c3d4e5f6'),
      at: { $gte: new Date("2026-01-01") },
      tags: [ 'a',  /b/i ],
      empty: {},
    }`;
    expect(compactQueryText(text)).toBe(
      `{ _id: ObjectId('65f0c3a2e4b0a1b2c3d4e5f6'), at: { $gte: new Date("2026-01-01") }, tags: ['a', /b/i], empty: {}, }`,
    );
  });

  it("returns null when the text can't be tokenized", () => {
    expect(compactQueryText("{ a: 'open")).toBeNull();
  });
});
