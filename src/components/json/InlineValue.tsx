import { inspectorText, valueTone } from "../../lib/bsonFormat";
import type { ValueTone } from "../../lib/bsonFormat";
import { useLegacyUuid } from "./LegacyUuidContext";

const TONE_CLASS: Record<ValueTone, string> = {
  string: "text-json-string",
  number: "text-json-number",
  keyword: "text-json-keyword",
  null: "text-json-keyword",
  bson: "text-json-bson",
  nested: "text-fg-3",
};

/** Break opportunities after the characters long identifiers split on. */
function withBreaks(text: string) {
  const parts = text.split(/(?<=[@./_-])/);
  return parts.map((part, i) => (
    <span key={i}>
      {part}
      {i < parts.length - 1 && <wbr />}
    </span>
  ));
}

interface InlineValueProps {
  value: unknown;
  /** Let long strings wrap at @ . / - _ (the inspector); off in one-line rows. */
  wrap?: boolean;
}

/**
 * A value as the inspector and tree show it: syntax-colored, without its
 * BSON wrapper (the row's type tag names the type), strings quoted.
 */
export function InlineValue({ value, wrap = true }: InlineValueProps) {
  const legacyUuid = useLegacyUuid();
  const tone = valueTone(value);
  if (tone === "string") {
    const text = value as string;
    return (
      <span className="text-json-string">
        "{wrap && text.length > 16 ? withBreaks(text) : text}"
      </span>
    );
  }
  return <span className={TONE_CLASS[tone]}>{inspectorText(value, legacyUuid)}</span>;
}
