import { beforeEach, describe, expect, it } from "vitest";
import { rangeBetween, useConnectionSelection } from "./connectionSelectionStore";

const order = ["a", "b", "c", "d", "e"];

describe("rangeBetween", () => {
  it("spans the anchor to the row, either way round", () => {
    expect(rangeBetween(order, "b", "d")).toEqual(["b", "c", "d"]);
    expect(rangeBetween(order, "d", "a")).toEqual(["a", "b", "c", "d"]);
  });

  it("is just the row without an anchor in view", () => {
    expect(rangeBetween(order, null, "c")).toEqual(["c"]);
    expect(rangeBetween(order, "gone", "c")).toEqual(["c"]);
  });

  it("is empty for a row not in view", () => {
    expect(rangeBetween(order, "a", "gone")).toEqual([]);
  });
});

describe("connection selection", () => {
  beforeEach(() => useConnectionSelection.getState().setSelecting(false));

  it("picks and un-picks a group at once, keeping the rest", () => {
    const s = useConnectionSelection.getState();
    s.toggle("a");
    s.setMany(["a", "b", "c"], true);
    expect(useConnectionSelection.getState().ids).toEqual(["a", "b", "c"]);
    s.setMany(["b", "c"], false);
    expect(useConnectionSelection.getState().ids).toEqual(["a"]);
  });

  it("clears the picks when select mode ends", () => {
    const s = useConnectionSelection.getState();
    s.setSelecting(true);
    s.toggle("a");
    s.setSelecting(false);
    expect(useConnectionSelection.getState()).toMatchObject({ ids: [], selecting: false });
  });

  it("toggles connections in and out", () => {
    const s = useConnectionSelection.getState();
    s.toggle("a");
    s.toggle("c");
    s.toggle("a");
    expect(useConnectionSelection.getState().ids).toEqual(["c"]);
  });

  it("ranges from the last toggled connection, keeping the anchor", () => {
    const s = useConnectionSelection.getState();
    s.toggle("b");
    s.selectRange("d", order);
    expect(useConnectionSelection.getState().ids).toEqual(["b", "c", "d"]);
    s.selectRange("a", order);
    expect(useConnectionSelection.getState().ids).toEqual(["a", "b"]);
  });
});
