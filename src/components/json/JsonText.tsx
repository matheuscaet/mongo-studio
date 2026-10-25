import { useMemo } from "react";
import { cn } from "@/lib/utils";
import { jsonTokens } from "../../lib/jsonText";
import type { JsonTone } from "../../lib/jsonText";
import { useLegacyUuid } from "./LegacyUuidContext";

const TONE_CLASS: Record<JsonTone, string | undefined> = {
  key: "text-json-key",
  string: "text-json-string",
  number: "text-json-number",
  keyword: "text-json-keyword",
  bson: "text-json-bson",
  punct: "text-json-punct",
  plain: undefined,
};

interface JsonTextProps {
  value: unknown;
  className?: string;
}

/** A value as pretty-printed, shell-like text with syntax colors; selectable. */
export function JsonText({ value, className }: JsonTextProps) {
  const legacyUuid = useLegacyUuid();
  const tokens = useMemo(() => jsonTokens(value, legacyUuid), [value, legacyUuid]);
  return (
    <pre
      className={cn(
        "m-0 px-3.5 py-2.5 font-data whitespace-pre text-fg select-text",
        className,
      )}
    >
      {tokens.map((token, i) =>
        token.tone === "plain" ? (
          token.text
        ) : (
          <span key={i} className={TONE_CLASS[token.tone]}>
            {token.text}
          </span>
        ),
      )}
    </pre>
  );
}
