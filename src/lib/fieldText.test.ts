import { describe, expect, it } from "vitest";
import { fieldNameError, fieldText, newObjectId, parseFieldText, valueAt } from "./fieldText";

/** Canonical Extended JSON → text → what goes back to the backend. */
function roundTrip(canonical: unknown): unknown {
  const parsed = parseFieldText(fieldText(canonical));
  if (!parsed.ok) throw new Error(parsed.error);
  return parsed.value;
}

describe("fieldText", () => {
  it("keeps every number's BSON type", () => {
    expect(fieldText({ $numberInt: "5" })).toBe("5");
    expect(roundTrip({ $numberInt: "5" })).toBe(5);
    expect(fieldText({ $numberLong: "5" })).toBe('NumberLong("5")');
    expect(roundTrip({ $numberLong: "9007199254740993" })).toEqual({ $numberLong: "9007199254740993" });
    expect(fieldText({ $numberDouble: "5.0" })).toBe("Double(5)");
    expect(roundTrip({ $numberDouble: "5.0" })).toEqual({ $numberDouble: "5" });
    expect(roundTrip({ $numberDouble: "2.5" })).toBe(2.5);
    expect(roundTrip({ $numberDouble: "0.1" })).toBe(0.1);
    expect(roundTrip({ $numberDouble: "1e+21" })).toEqual({ $numberDouble: "1e+21" });
    expect(roundTrip({ $numberDouble: "NaN" })).toEqual({ $numberDouble: "NaN" });
    expect(roundTrip({ $numberDouble: "Infinity" })).toEqual({ $numberDouble: "Infinity" });
    expect(roundTrip({ $numberDouble: "-Infinity" })).toEqual({ $numberDouble: "-Infinity" });
    expect(roundTrip({ $numberDouble: "-0.0" })).toEqual({ $numberDouble: "-0.0" });
    expect(roundTrip({ $numberDecimal: "10.250" })).toEqual({ $numberDecimal: "10.250" });
  });

  it("writes BSON wrappers as shell helpers that read back the same", () => {
    expect(roundTrip({ $oid: "65f0c3a2e4b0a1b2c3d4e5f6" })).toEqual({ $oid: "65f0c3a2e4b0a1b2c3d4e5f6" });
    expect(fieldText({ $date: { $numberLong: "1767225600000" } })).toBe('ISODate("2026-01-01T00:00:00.000Z")');
    expect(roundTrip({ $date: { $numberLong: "1767225600000" } })).toEqual({ $date: "2026-01-01T00:00:00.000Z" });
    const uuid = { $binary: { base64: "OyQRAeK7QlWMr0E2xWapYg==", subType: "04" } };
    expect(fieldText(uuid)).toBe('UUID("3b241101-e2bb-4255-8caf-4136c566a962")');
    expect(roundTrip(uuid)).toEqual({ $uuid: "3b241101-e2bb-4255-8caf-4136c566a962" });
    // long binary data stays whole (the display text shortens it)
    const long = "QUJDREVGR0hJSktMTU5PUFFSU1RVVldYWVo=";
    expect(roundTrip({ $binary: { base64: long, subType: "00" } })).toEqual({ $binary: { base64: long, subType: "00" } });
    expect(roundTrip({ $binary: { base64: "AAEC", subType: "80" } })).toEqual({ $binary: { base64: "AAEC", subType: "80" } });
    expect(roundTrip({ $timestamp: { t: 1700000000, i: 3 } })).toEqual({ $timestamp: { t: 1700000000, i: 3 } });
    expect(roundTrip({ $regularExpression: { pattern: "^ac", options: "i" } })).toEqual({
      $regularExpression: { pattern: "^ac", options: "i" },
    });
    expect(roundTrip({ $minKey: 1 })).toEqual({ $minKey: 1 });
    expect(roundTrip({ $maxKey: 1 })).toEqual({ $maxKey: 1 });
  });

  it("falls back to Extended JSON for what the shell syntax can't say", () => {
    const re = { $regularExpression: { pattern: "a/b", options: "" } };
    expect(roundTrip(re)).toEqual(re);
    const farDate = { $date: { $numberLong: "-62198755200001" } };
    expect(roundTrip(farDate)).toEqual(farDate);
    const code = { $code: "function () { return 1 }" };
    expect(roundTrip(code)).toEqual(code);
  });

  it("pretty-prints arrays and objects, quoting keys that need it", () => {
    const value = {
      sku: "A",
      "unit price": { $numberDouble: "5.0" },
      tags: [],
      dims: {},
      items: [{ qty: { $numberInt: "2" } }, null, true],
    };
    expect(fieldText(value)).toBe(
      [
        "{",
        '  sku: "A",',
        '  "unit price": Double(5),',
        "  tags: [],",
        "  dims: {},",
        "  items: [",
        "    {",
        "      qty: 2",
        "    },",
        "    null,",
        "    true",
        "  ]",
        "}",
      ].join("\n"),
    );
    expect(roundTrip(value)).toEqual({
      sku: "A",
      "unit price": { $numberDouble: "5" },
      tags: [],
      dims: {},
      items: [{ qty: 2 }, null, true],
    });
    expect(roundTrip("he said \"hi\"\n")).toBe('he said "hi"\n');
  });
});

describe("parseFieldText", () => {
  it("reads what the user types, mongosh style", () => {
    expect(parseFieldText("[]")).toEqual({ ok: true, value: [] });
    expect(parseFieldText("{ a: 1, }")).toEqual({ ok: true, value: { a: 1 } });
    expect(parseFieldText("'x'")).toEqual({ ok: true, value: "x" });
  });

  it("says where the text is wrong, and refuses an empty value", () => {
    const bad = parseFieldText("[1, 2");
    expect(bad.ok).toBe(false);
    expect(!bad.ok && bad.error).toMatch(/line 1/);
    expect(parseFieldText("  ").ok).toBe(false);
  });
});

describe("fieldNameError", () => {
  it("mirrors the backend's rule", () => {
    expect(fieldNameError("note")).toBeNull();
    expect(fieldNameError("")).not.toBeNull();
    expect(fieldNameError("a.b")).not.toBeNull();
    expect(fieldNameError("$x")).not.toBeNull();
  });
});

describe("valueAt / newObjectId", () => {
  it("follows objects and array items", () => {
    const doc = { items: [{ sku: "A" }, { tags: ["x"] }], a: { b: 1 } };
    expect(valueAt(doc, ["items", "1", "tags", "0"])).toBe("x");
    expect(valueAt(doc, ["a", "b"])).toBe(1);
    expect(valueAt(doc, ["a", "b", "c"])).toBeUndefined();
    expect(valueAt(doc, [])).toBe(doc);
  });

  it("makes 24-hex ObjectIds that start with the time", () => {
    const a = newObjectId(1767225600000);
    const b = newObjectId(1767225600000);
    expect(a).toMatch(/^[0-9a-f]{24}$/);
    expect(a.slice(0, 8)).toBe((1767225600).toString(16));
    expect(a).not.toBe(b);
  });
});
