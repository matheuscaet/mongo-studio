/** What MCP clients may do; each is Off, Ask or Allow in Settings. */
export type McpCapability =
  | "browse"
  | "read"
  | "write"
  | "ddl"
  | "script"
  | "connList"
  | "connEdit"
  | "connDelete"
  | "saveScript"
  | "relRead"
  | "relEdit";

export type McpPolicy = "off" | "ask" | "allow";

/** A saved connection's access: none, read, or read & write. */
export type McpLevel = "none" | "read" | "write";

/** What `mcp_set_config` takes. */
export interface McpConfig {
  enabled: boolean;
  port: number;
  caps: Partial<Record<McpCapability, McpPolicy>>;
  connections: Record<string, McpLevel>;
  /** Per connection id, or "*" for capabilities that aren't about one. */
  always: Record<string, McpCapability[]>;
  /** How the app calls the user back when a request waits. */
  alerts: { sound: boolean; focus: boolean };
}

export interface McpClient {
  id: string;
  name: string;
  version: string | null;
  /** Milliseconds since the Unix epoch. */
  connectedAt: number;
  lastSeen: number;
}

export interface McpStatus {
  running: boolean;
  port: number;
  url: string;
  /** Why it isn't running when it should, e.g. the port is taken. */
  error: string | null;
  clients: McpClient[];
}

/** An AI client the server can be added to with one click. */
export interface McpClientTarget {
  id: "claude" | "cursor" | "codex" | "vscode" | "windsurf";
  name: string;
  installed: boolean;
  registered: boolean;
  configPath: string | null;
}

/** Payload of `mcp-request`: a call set to Ask, waiting for the user. */
export interface McpRequest {
  requestId: string;
  clientId: string;
  clientName: string;
  tool: string;
  capability: McpCapability;
  /** Null for calls that aren't about one connection, e.g. create_connection. */
  connectionId: string | null;
  connectionName: string | null;
  database: string | null;
  collection: string | null;
  /** e.g. "updateMany on shop.users" */
  title: string;
  /** The statement, as mongosh-style text. */
  code: string;
  /** e.g. "12 documents would change" */
  dryRun: string | null;
  /** Milliseconds since the Unix epoch. */
  at: number;
}

/** Payload of `mcp-activity`: one tool call and how it ended. */
export interface McpActivity {
  at: number;
  clientName: string;
  tool: string;
  target: string;
  connectionId: string | null;
  meta: string;
  result: "ok" | "warn" | "err";
  /** e.g. "Allowed", "Approved by you", "Refused: no access" */
  label: string;
}

export type McpRequestChoice = "once" | "always" | "deny";

/** Payload of `mcp-connection`: a client created, changed or deleted a saved connection. */
export interface McpConnectionChange {
  change: "created" | "updated" | "deleted";
  connectionId: string;
  name: string;
  folderId: string | null;
  /** On "created": the level it starts at ("read"). */
  level: McpLevel | null;
}
