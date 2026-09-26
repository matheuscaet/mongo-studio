import { useMemo } from "react";
import { useSessionsStore } from "../../store/sessionsStore";
import type { DatabaseRef } from "../../store/sessionsStore";
import type { CollectionInfo } from "../../types/connection";

export interface CollectionMatches {
  /** Matching collections by `databaseKey`. */
  byDatabase: Map<string, CollectionInfo[]>;
  /** Connections holding a match, which stay in view while searching. */
  connectionIds: Set<string>;
  /** Where Enter in the search goes. */
  first: DatabaseRef & { collection: string };
}

/**
 * Collections whose names contain the sidebar search, or null when none
 * do. It covers every database whose collections the sidebar has listed,
 * on any connection - the others would cost a round trip each to search.
 */
export function useCollectionMatches(query: string): CollectionMatches | null {
  const databaseTree = useSessionsStore((s) => s.databaseTree);

  return useMemo(() => {
    if (!query) return null;
    const byDatabase = new Map<string, CollectionInfo[]>();
    const connectionIds = new Set<string>();
    let first: CollectionMatches["first"] | null = null;
    for (const [key, tree] of Object.entries(databaseTree)) {
      if (!tree.loaded) continue;
      const matching = tree.collections.filter((c) => c.name.toLowerCase().includes(query));
      if (matching.length === 0) continue;
      // keys are "<connection id>/<database>", and ids hold no "/"
      const slash = key.indexOf("/");
      const connectionId = key.slice(0, slash);
      byDatabase.set(key, matching);
      connectionIds.add(connectionId);
      first ??= { connectionId, database: key.slice(slash + 1), collection: matching[0].name };
    }
    return first ? { byDatabase, connectionIds, first } : null;
  }, [query, databaseTree]);
}
