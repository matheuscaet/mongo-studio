import { describe, expect, it } from "vitest";
import { clearPicks, pickedIndexesOf, pickRow, selectAllOf, selectedDocumentOf, selectRow } from "./useDocumentSelection";

const docs = (n: number) => Array.from({ length: n }, (_, i) => ({ _id: { $oid: String(i).padStart(24, "0") } }));

describe("document selection", () => {
  it("starts at the first row, alone", () => {
    const page = docs(5);
    expect(pickedIndexesOf("t0", page)).toEqual([0]);
    expect(selectedDocumentOf("t0", page)).toBe(page[0]);
  });

  it("toggles rows, picks ranges from the anchor and clears back to it", () => {
    const page = docs(8);
    selectRow("t1", page, 2);
    pickRow("t1", page, 5, "toggle");
    pickRow("t1", page, 4, "toggle");
    expect(pickedIndexesOf("t1", page)).toEqual([2, 4, 5]);
    pickRow("t1", page, 5, "toggle");
    expect(pickedIndexesOf("t1", page)).toEqual([2, 4]);
    pickRow("t1", page, 6, "range");
    expect(pickedIndexesOf("t1", page)).toEqual([2, 3, 4, 5, 6]);
    pickRow("t1", page, 0, "range");
    expect(pickedIndexesOf("t1", page)).toEqual([0, 1, 2]);
    expect(selectedDocumentOf("t1", page)).toBe(page[2]);
    clearPicks("t1");
    expect(pickedIndexesOf("t1", page)).toEqual([2]);
  });

  it("hands the anchor on when it is un-picked", () => {
    const page = docs(6);
    selectRow("t2", page, 1);
    pickRow("t2", page, 3, "toggle");
    pickRow("t2", page, 4, "toggle");
    pickRow("t2", page, 1, "toggle");
    expect(pickedIndexesOf("t2", page)).toEqual([3, 4]);
    expect(selectedDocumentOf("t2", page)).toBe(page[3]);
    // the last row picked can't be un-picked: something stays selected
    pickRow("t2", page, 4, "toggle");
    pickRow("t2", page, 3, "toggle");
    expect(pickedIndexesOf("t2", page)).toEqual([3]);
  });

  it("selects every row of the page", () => {
    const page = docs(4);
    selectRow("t3", page, 1);
    selectAllOf("t3", page);
    expect(pickedIndexesOf("t3", page)).toEqual([0, 1, 2, 3]);
    expect(selectedDocumentOf("t3", page)).toBe(page[1]);
  });

  it("follows the picked documents into new results, by _id", () => {
    const page = docs(6);
    selectRow("t4", page, 1);
    pickRow("t4", page, 3, "toggle");
    // an edit replaced document 3 with a copy; the delete took document 0 away
    const next = [page[1], page[2], { ...page[3], edited: true }, page[4], page[5]];
    expect(pickedIndexesOf("t4", next)).toEqual([0, 2]);
    expect(selectedDocumentOf("t4", next)).toBe(page[1]);
  });
});
