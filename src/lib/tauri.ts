import { invoke } from "@tauri-apps/api/core";
import type {
  CollectionInfo,
  ConnectionHandle,
  ConnectionProfile,
  ConnectionProfileInput,
  ConnectionProfileMeta,
  LegacyUuidEncoding,
  ConnectionImportPreview,
  ConnectionTestResult,
  ConnectionsExportSummary,
  ConnectionsImportSummary,
  DatabaseInfo,
  SecretBackendInfo,
} from "../types/connection";
import type {
  CollectionStats,
  FindQueryInput,
  QueryResultPage,
} from "../types/query";
import type { MongoshInfo, MongoshRunOptions, SavedScript, ScriptResult } from "../types/script";
import type { McpClientTarget, McpConfig, McpRequestChoice, McpStatus } from "../types/mcp";
import type {
  FindRelatedInput,
  FindRelatedOutput,
  Relation,
  RelationCheckReport,
  RelationDir,
  RelationSuggestion,
} from "../types/relations";
import type { ExportOptions, ExportQueryInput, ExportSummary } from "../types/export";
import type { ExplainQueryInput, ExplainVerbosity } from "../types/explain";
import type {
  ApprovalChoice,
  AssistantPolicy,
  AssistantStartInput,
  DetectedAgent,
} from "../types/assistant";

export const api = {
  listConnectionProfiles: () =>
    invoke<ConnectionProfileMeta[]>("list_connection_profiles"),

  getConnectionProfile: (id: string) =>
    invoke<ConnectionProfile>("get_connection_profile", { id }),

  saveConnectionProfile: (input: ConnectionProfileInput) =>
    invoke<ConnectionProfileMeta>("save_connection_profile", { input }),

  setLegacyUuidEncoding: (id: string, encoding: LegacyUuidEncoding | null) =>
    invoke<ConnectionProfileMeta>("set_legacy_uuid_encoding", { id, encoding }),

  deleteConnectionProfile: (id: string) =>
    invoke<void>("delete_connection_profile", { id }),

  testConnection: (input: ConnectionProfileInput) =>
    invoke<ConnectionTestResult>("test_connection", { input }),

  connect: (id: string) => invoke<ConnectionHandle>("connect", { id }),

  disconnect: (sessionId: string) =>
    invoke<void>("disconnect", { sessionId }),

  secretBackendInfo: () =>
    invoke<SecretBackendInfo>("secret_backend_info"),

  /** Opens the app's log folder in the file manager; resolves to its path. */
  openLogDir: () => invoke<string>("open_log_dir"),

  /** Exports the connections in `ids`, or every saved one when null. */
  exportConnections: (destPath: string, includeSecrets: boolean, ids: string[] | null = null) =>
    invoke<ConnectionsExportSummary>("export_connections", {
      destPath,
      includeSecrets,
      ids,
    }),

  previewConnectionsImport: (srcPath: string) =>
    invoke<ConnectionImportPreview[]>("preview_connections_import", { srcPath }),

  /** Imports the connections at `selected` positions of the file. */
  importConnections: (srcPath: string, selected: number[]) =>
    invoke<ConnectionsImportSummary>("import_connections", { srcPath, selected }),

  listDatabases: (sessionId: string) =>
    invoke<DatabaseInfo[]>("list_databases", { sessionId }),

  listCollections: (sessionId: string, database: string) =>
    invoke<CollectionInfo[]>("list_collections", { sessionId, database }),

  getCollectionStats: (sessionId: string, database: string, collection: string) =>
    invoke<CollectionStats>("get_collection_stats", { sessionId, database, collection }),

  runFind: (
    sessionId: string,
    database: string,
    collection: string,
    query: FindQueryInput,
    /** Lets cancelQuery stop it. */
    executionId: string | null = null,
  ) => invoke<QueryResultPage>("run_find", { sessionId, database, collection, query, executionId }),

  /** Sets one field of the document with this _id; returns it as stored. */
  updateField: (
    sessionId: string,
    database: string,
    collection: string,
    id: unknown,
    path: string[],
    value: unknown,
  ) =>
    invoke<unknown>("update_field", { sessionId, database, collection, id, path, value }),

  runAggregate: (
    sessionId: string,
    database: string,
    collection: string,
    pipeline: unknown,
    /** Lets cancelQuery stop it. */
    executionId: string | null = null,
  ) =>
    invoke<QueryResultPage>("run_aggregate", {
      sessionId,
      database,
      collection,
      pipeline,
      executionId,
    }),

  countDocuments: (
    sessionId: string,
    database: string,
    collection: string,
    filter: unknown,
    /** Lets cancelQuery stop it. */
    executionId: string | null = null,
  ) => invoke<number>("count_documents", { sessionId, database, collection, filter, executionId }),

  /** Stops a find, aggregate or count started with this execution id. */
  cancelQuery: (executionId: string) => invoke<void>("cancel_query", { executionId }),

  runScript: (
    sessionId: string,
    database: string,
    script: string,
    executionId: string,
    timeoutMs: number | null,
    engine: "builtin" | "mongosh" = "builtin",
    mongoshOptions: MongoshRunOptions | null = null,
  ) =>
    invoke<ScriptResult>("run_script", {
      sessionId,
      database,
      script,
      executionId,
      timeoutMs,
      engine,
      mongoshOptions,
    }),

  cancelScript: (executionId: string) =>
    invoke<void>("cancel_script", { executionId }),

  /** The sidebar's folder tree as stored, or null before one was saved. */
  getSidebarLayout: () => invoke<unknown>("get_sidebar_layout"),

  saveSidebarLayout: (layout: unknown) => invoke<void>("save_sidebar_layout", { layout }),

  suggestScriptPath: () => invoke<string>("suggest_script_path"),

  /** With a null path the backend picks a random name in the default folder. */
  saveScript: (path: string | null, content: string) =>
    invoke<SavedScript>("save_script", { path, content }),

  listSavedScripts: () => invoke<SavedScript[]>("list_saved_scripts"),

  readSavedScript: (path: string) => invoke<string>("read_saved_script", { path }),

  /** Streams a whole find or aggregation to a file. */
  exportQuery: (
    sessionId: string,
    database: string,
    collection: string,
    query: ExportQueryInput,
    options: ExportOptions,
    destPath: string,
    executionId: string,
  ) =>
    invoke<ExportSummary>("export_query", {
      sessionId,
      database,
      collection,
      query,
      options,
      destPath,
      executionId,
    }),

  /** Writes a value already in hand, such as a console result, to a file. */
  exportValue: (value: unknown, options: ExportOptions, destPath: string) =>
    invoke<ExportSummary>("export_value", { value, options, destPath }),

  cancelExport: (executionId: string) =>
    invoke<void>("cancel_export", { executionId }),

  listIndexStats: (sessionId: string, database: string, collection: string) =>
    invoke<unknown[]>("list_index_stats", { sessionId, database, collection }),

  explainQuery: (
    sessionId: string,
    database: string,
    collection: string,
    query: ExplainQueryInput,
    verbosity: ExplainVerbosity,
  ) =>
    invoke<unknown>("explain_query", {
      sessionId,
      database,
      collection,
      query,
      verbosity,
    }),

  /** Finds the Claude Code and Codex CLIs on this machine. */
  assistantDetect: () => invoke<DetectedAgent[]>("assistant_detect"),

  assistantSetPolicy: (policy: AssistantPolicy) =>
    invoke<void>("assistant_set_policy", { policy }),

  /** Binds an agent session to one database; nothing runs until a send. */
  assistantStart: (input: AssistantStartInput) =>
    invoke<{ agentSessionId: string }>("assistant_start", { input }),

  /** Starts a turn; its text and steps arrive as events. */
  assistantSend: (agentSessionId: string, text: string) =>
    invoke<void>("assistant_send", { agentSessionId, text }),

  assistantStop: (agentSessionId: string) =>
    invoke<void>("assistant_stop", { agentSessionId }),

  assistantClose: (agentSessionId: string) =>
    invoke<void>("assistant_close", { agentSessionId }),

  assistantAnswer: (requestId: string, choice: ApprovalChoice) =>
    invoke<void>("assistant_answer", { requestId, choice }),

  /** Where the agents run, for resuming a session in a terminal. */
  assistantWorkdir: () => invoke<string>("assistant_workdir"),

  /** Finds mongosh: the picked executable, a downloaded copy, then the PATH. */
  mongoshDetect: (customPath: string | null) => invoke<MongoshInfo>("mongosh_detect", { customPath }),

  /** Downloads MongoDB's mongosh build; progress arrives as `mongosh-download` events. */
  mongoshDownload: () => invoke<MongoshInfo>("mongosh_download"),

  /** Applies the MCP server's settings: starts, stops or restarts it. */
  mcpSetConfig: (config: McpConfig) => invoke<McpStatus>("mcp_set_config", { config }),
  mcpStatus: () => invoke<McpStatus>("mcp_status"),
  mcpToken: () => invoke<string>("mcp_token"),
  mcpRegenerateToken: () => invoke<string>("mcp_regenerate_token"),
  mcpAnswer: (requestId: string, choice: McpRequestChoice) =>
    invoke<void>("mcp_answer", { requestId, choice }),
  mcpDisconnectClient: (clientId: string) => invoke<void>("mcp_disconnect_client", { clientId }),
  mcpClientTargets: () => invoke<McpClientTarget[]>("mcp_client_targets"),
  mcpRegisterClient: (id: string) => invoke<McpClientTarget>("mcp_register_client", { id }),
  mcpUnregisterClient: (id: string) => invoke<McpClientTarget>("mcp_unregister_client", { id }),

  relationsList: () => invoke<Relation[]>("relations_list"),
  relationSave: (relation: Relation) => invoke<Relation>("relation_save", { relation }),
  relationDelete: (id: string) => invoke<void>("relation_delete", { id }),
  /** The connection the other side lives on for documents of this connection, or null. */
  relationTargetConnection: (relation: Relation, dir: RelationDir, sourceConnectionId: string) =>
    invoke<string | null>("relation_target_connection", { relation, dir, sourceConnectionId }),
  /** The related documents of a page of documents; one query per rule on the other side. */
  relationFindRelated: (input: FindRelatedInput) => invoke<FindRelatedOutput>("relation_find_related", { input }),
  relationSuggestLinks: (input: {
    fromSessionId: string;
    from: { database: string; collection: string };
    toSessionId: string;
    to: { database: string; collection: string };
  }) => invoke<RelationSuggestion[]>("relation_suggest_links", { input }),
  relationCheck: (input: { relation: Relation; fromConnectionId: string; fromSessionId: string; toSessionId: string }) =>
    invoke<RelationCheckReport>("relation_check", { input }),
};
