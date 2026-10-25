import { createContext, useContext } from "react";
import type { LegacyUuidEncoding } from "../../lib/bsonValue";

/** How the values below read legacy UUIDs: their tab's connection's choice. */
export const LegacyUuidContext = createContext<LegacyUuidEncoding>("raw");

export function useLegacyUuid(): LegacyUuidEncoding {
  return useContext(LegacyUuidContext);
}
