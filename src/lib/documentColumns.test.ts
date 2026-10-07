import { describe, expect, it } from "vitest";
import { columnsOf, orderFields } from "./documentColumns";

describe("columnsOf", () => {
  it("puts _id first and double-underscore fields last", () => {
    const docs = [
      { __v: 0, key: { $binary: { base64: "AAAAAAAAAAAAAAAAAAAAAA==", subType: "04" } }, _id: { $oid: "65f0c3a2e4b0a1b2c3d4e5f6" }, name: "a" },
      { owner: { $oid: "65f0c3a2e4b0a1b2c3d4e5f7" }, meta: { a: 1 }, __t: "x" },
    ];
    expect(columnsOf(docs).map((c) => c.key)).toEqual(["_id", "key", "name", "meta", "owner", "__v", "__t"]);
  });

  it("names a UUID column as such", () => {
    const docs = [{ key: { $binary: { base64: "GOisuxSzQW2wdNzVmwWH3w==", subType: "04" } } }];
    expect(columnsOf(docs)[0].type).toBe("UUID");
  });
});

describe("orderFields", () => {
  it("moves _id to the front and __ fields to the end, keeping the rest as stored", () => {
    const entries: [string, number][] = [["__v", 0], ["b", 1], ["_id", 2], ["a", 3], ["__t", 4]];
    expect(orderFields(entries).map(([k]) => k)).toEqual(["_id", "b", "a", "__v", "__t"]);
  });
});
