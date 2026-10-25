import { useMemo } from "react";
import { X } from "lucide-react";
import { detectLegacyUuid } from "../../lib/bsonValue";
import { useConnectionsStore } from "../../store/connectionsStore";
import type { CollectionTab } from "../../store/sessionsStore";

const NAMES = { python: "Python", java: "Java", csharp: "C#" } as const;

/**
 * Offers to read the results' legacy UUIDs in the byte order that wrote
 * them, while the connection still shows them raw and nobody picked that.
 * Picking either way saves it on the connection, so it asks once.
 */
export function LegacyUuidHint({ tab }: { tab: CollectionTab }) {
  const connectionId = tab.connection.id;
  const undecided = useConnectionsStore((s) => {
    const profile = s.profiles.find((p) => p.id === connectionId);
    return profile !== undefined && profile.legacyUuid === "raw" && !profile.legacyUuidChosen;
  });
  const setLegacyUuid = useConnectionsStore((s) => s.setLegacyUuid);
  const documents = tab.results?.documents;
  const guess = useMemo(
    () => (undecided && documents ? detectLegacyUuid(documents) : null),
    [undecided, documents],
  );
  if (!guess) return null;

  const name = NAMES[guess.encoding];
  return (
    <span className="flex h-[22px] min-w-0 items-center rounded-full bg-warn/13 text-xs text-warn">
      <button
        type="button"
        className="min-w-0 truncate rounded-l-full py-0.5 pr-1.5 pl-2.5 hover:bg-warn/10"
        title={`${guess.matched} of ${guess.sampled} binary subtype 3 values read as valid UUIDs in ${name} byte order. Change it any time in the connection's Advanced settings.`}
        onClick={() => setLegacyUuid(connectionId, guess.encoding)}
      >
        Looks like {name} legacy UUIDs · Show as {name}
      </button>
      <button
        type="button"
        className="grid h-full place-items-center rounded-r-full pr-2 pl-1 hover:bg-warn/10"
        aria-label="Keep showing raw BinData"
        title="Keep showing raw BinData"
        onClick={() => setLegacyUuid(connectionId, "raw")}
      >
        <X className="size-3" />
      </button>
    </span>
  );
}
