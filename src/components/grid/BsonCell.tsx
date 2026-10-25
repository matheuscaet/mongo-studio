import { cellText, objectIdHex, valueTone } from "../../lib/bsonFormat";
import type { ValueTone } from "../../lib/bsonFormat";
import { useLegacyUuid } from "../json/LegacyUuidContext";

/** In the grid plain strings stay in body text; only other types take color. */
const TONE_CLASS: Record<ValueTone, string> = {
  string: "text-fg",
  number: "text-json-number",
  keyword: "text-json-keyword",
  null: "text-fg-3 italic",
  bson: "text-json-bson",
  nested: "text-fg-3",
};

/** A value as a grid cell shows it; nothing for a field the document lacks. */
export function BsonCell({ value }: { value: unknown }) {
  const legacyUuid = useLegacyUuid();
  if (value === undefined) return null;
  const hex = objectIdHex(value);
  if (hex !== null && hex.length > 12) {
    // head…tail: enough to tell ids apart; the cell's title has it whole
    return (
      <span className="text-json-bson">
        {hex.slice(0, 6)}
        <span className="text-fg-3">…</span>
        {hex.slice(-5)}
      </span>
    );
  }
  return <span className={TONE_CLASS[valueTone(value)]}>{cellText(value, legacyUuid)}</span>;
}
