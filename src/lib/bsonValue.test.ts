import { describe, expect, it } from "vitest";
import { bsonLiteral, bsonTypeName, detectLegacyUuid, legacyUuidText, uuidText } from "./bsonValue";
import { cellText, shortTypeName } from "./bsonFormat";

/** 18e8acbb-14b3-416d-b074-dcd59b0587df as the backend sends it. */
const uuid = { $binary: { base64: "GOisuxSzQW2wdNzVmwWH3w==", subType: "04" } };

describe("UUIDs", () => {
  it("reads a subtype 4 binary as its dashed UUID", () => {
    expect(uuidText(uuid)).toBe("18e8acbb-14b3-416d-b074-dcd59b0587df");
    expect(bsonLiteral(uuid)).toBe('UUID("18e8acbb-14b3-416d-b074-dcd59b0587df")');
    expect(cellText(uuid)).toBe("18e8acbb-14b3-416d-b074-dcd59b0587df");
    expect(bsonTypeName(uuid)).toBe("UUID");
    expect(shortTypeName(uuid)).toBe("uuid");
  });

  it("leaves other binaries as BinData", () => {
    const generic = { $binary: { base64: "GOisuxSzQW2wdNzVmwWH3w==", subType: "00" } };
    const short = { $binary: { base64: "AAEC", subType: "04" } };
    expect(uuidText(generic)).toBeNull();
    expect(uuidText(short)).toBeNull();
    expect(bsonLiteral(generic)).toBe('BinData(00, "GOisuxSzQW2wdNzVmwWH3w==")');
    expect(bsonTypeName(short)).toBe("Binary");
  });
});

/** 6513270e-269e-4d37-b2a7-4de452e6b438 as the old Java driver stored it. */
const legacy = (base64: string) => ({ $binary: { base64, subType: "03" } });
const javaStored = legacy("N02eJg4nE2U4tOZS5E2nsg==");

describe("legacy UUIDs", () => {
  it("reads subtype 3 in each driver's byte order", () => {
    expect(legacyUuidText(javaStored, "java")).toBe("6513270e-269e-4d37-b2a7-4de452e6b438");
    expect(legacyUuidText(javaStored, "csharp")).toBe("269e4d37-270e-6513-38b4-e652e44da7b2");
    expect(legacyUuidText(javaStored, "python")).toBe("374d9e26-0e27-1365-38b4-e652e44da7b2");
    expect(legacyUuidText(javaStored, "raw")).toBeNull();
    expect(legacyUuidText(uuid, "java")).toBeNull();
  });

  it("shows them as constructors a filter reads back, and raw by default", () => {
    expect(bsonLiteral(javaStored, "java")).toBe('LegacyJavaUUID("6513270e-269e-4d37-b2a7-4de452e6b438")');
    expect(bsonLiteral(javaStored)).toBe('BinData(03, "N02eJg4nE2U4tOZS5E2nsg==")');
    expect(bsonTypeName(javaStored, false, "csharp")).toBe("LegacyUUID");
    expect(shortTypeName(javaStored, "java")).toBe("uuid·3");
    expect(cellText(javaStored, "java")).toBe("6513270e-269e-4d37-b2a7-4de452e6b438");
  });

  it("detects the byte order from the version digits", () => {
    const docs = ["N02eJg4nE2U4tOZS5E2nsg==", "M0+LEiQIP9JQpKOm0H9cjA==", "+EmdXV2YMZUrkC+JEegYmA==", "9U7ngcx19jZ1BNkOlF3iqA=="].map(
      (b64, i) => ({ _id: i, ref: { customerId: legacy(b64) } }),
    );
    expect(detectLegacyUuid(docs)).toEqual({ encoding: "java", matched: 4, sampled: 4 });
    expect(detectLegacyUuid(docs.slice(0, 2))).toBeNull();
    expect(detectLegacyUuid([{ a: uuid }, { a: uuid }, { a: uuid }])).toBeNull();
  });
});
