import { describe, expect, it } from "vitest";
import { bsonLiteral, bsonTypeName, uuidText } from "./bsonValue";
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
