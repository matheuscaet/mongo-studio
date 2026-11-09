import { useEffect, useState } from "react";
import { Bot, Check, Copy, Eye, EyeOff, Lock, Sparkles, Volume2, X } from "lucide-react";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import { ConnectionChip } from "@/components/ui/ConnectionChip";
import { Input } from "@/components/ui/input";
import { SegmentedControl, type SegmentOption } from "@/components/ui/segmented-control";
import { connectionColor } from "@/lib/connectionColor";
import { api } from "@/lib/tauri";
import { playChime } from "@/lib/attention";
import { cn } from "@/lib/utils";
import { useConnectionsStore } from "../../store/connectionsStore";
import { useMcpStore } from "../../store/mcpStore";
import { DEFAULT_MCP_PORT, useSettingsStore, type McpSettings as McpSettingsValue } from "../../store/settingsStore";
import { useUiStore } from "../../store/uiStore";
import type { McpCapability, McpClientTarget, McpLevel, McpPolicy } from "../../types/mcp";
import { ConnectionPickerHeader, ConnectionTree, ConnectionTreeEmpty, useConnectionTree } from "../connections/ConnectionTreePicker";
import { Field } from "../connections/Field";
import { SectionTitle } from "../connections/SectionTitle";
import { SwitchField } from "../connections/SwitchField";
import { Aside, GroupRow, Note, RowGroup } from "./parts";

type CapId = keyof McpSettingsValue["caps"];

interface CapRow {
  id: CapId | "browse";
  label: string;
  help?: string;
}

const CAP_GROUPS: { title: string; rows: CapRow[] }[] = [
  {
    title: "Read",
    rows: [
      { id: "browse", label: "List databases, collections and indexes", help: "Names, counts and index definitions. Everything else needs it." },
      { id: "read", label: "Read documents", help: "find, aggregate, count and explain, capped at 100 documents per call." },
    ],
  },
  {
    title: "Write",
    rows: [
      { id: "write", label: "Insert, update and delete documents", help: "Each request shows what it changes and a dry-run count before it runs." },
      { id: "ddl", label: "Create and drop collections and indexes" },
      { id: "script", label: "Run console scripts", help: "A script can do anything the connection's database user can." },
    ],
  },
  {
    title: "Mongo Studio",
    rows: [
      { id: "connList", label: "See saved connections", help: "Name, folder and host. Never passwords or full connection strings." },
      { id: "connEdit", label: "Create and edit saved connections", help: "Passwords for new connections are typed by you, in the app." },
      { id: "connDelete", label: "Delete saved connections" },
      { id: "saveScript", label: "Save queries and scripts", help: "They land in Saved scripts, marked with the client's name." },
      { id: "relRead", label: "See relations and find related documents", help: "Related documents are read like any other, on connections it may read." },
      { id: "relEdit", label: "Propose and test relations", help: "What a client creates is a draft until you save it from its tab." },
    ],
  },
];

const CAP_LABELS: Record<McpCapability, string> = {
  browse: "Browse",
  read: "Read documents",
  write: "Writes",
  ddl: "Collections and indexes",
  script: "Scripts",
  connList: "See connections",
  connEdit: "Edit connections",
  connDelete: "Delete connections",
  saveScript: "Save scripts",
  relRead: "Relations",
  relEdit: "Propose relations",
};

const POLICY_TONE: Record<McpPolicy, string> = { off: "", ask: "text-warn", allow: "text-ok" };

function policyOptions(locked: boolean): SegmentOption<McpPolicy>[] {
  return (["off", "ask", "allow"] as McpPolicy[]).map((value) => ({
    value,
    label: value === "off" ? "Off" : value === "ask" ? "Ask" : "Allow",
    pressedClassName: POLICY_TONE[value],
    disabled: locked && value !== "allow",
  }));
}

const LEVEL_OPTIONS: SegmentOption<McpLevel>[] = [
  { value: "none", label: "None" },
  { value: "read", label: "Read" },
  { value: "write", label: "Read & write", pressedClassName: "text-warn" },
];

/** The client config snippets, per client, as text. */
function snippet(client: string, url: string, token: string): string {
  const auth = `Bearer ${token}`;
  switch (client) {
    case "claude":
      return `# in a terminal\nclaude mcp add --transport http --scope user mongo-studio ${url} \\\n  --header "Authorization: ${auth}"`;
    case "cursor":
      return `// ~/.cursor/mcp.json\n${JSON.stringify({ mcpServers: { "mongo-studio": { url, headers: { Authorization: auth } } } }, null, 2)}`;
    case "codex":
      return `# ~/.codex/config.toml\n[mcp_servers.mongo-studio]\nurl = "${url}"\n\n[mcp_servers.mongo-studio.http_headers]\nAuthorization = "${auth}"`;
    case "vscode":
      return `// VS Code: MCP: Open User Configuration\n${JSON.stringify({ servers: { "mongo-studio": { type: "http", url, headers: { Authorization: auth } } } }, null, 2)}`;
    default:
      return JSON.stringify({ type: "http", url, headers: { Authorization: auth } }, null, 2);
  }
}

const SNIPPET_CLIENTS = [
  { value: "claude", label: "Claude Code" },
  { value: "cursor", label: "Cursor" },
  { value: "codex", label: "Codex" },
  { value: "vscode", label: "VS Code" },
  { value: "json", label: "JSON" },
];

const masked = (token: string) => (token ? `${token.slice(0, 4)}••••••••${token.slice(-4)}` : "");

function copy(text: string, what: string) {
  navigator.clipboard.writeText(text).then(
    () => toast(`Copied the ${what}`),
    () => toast.error(`Couldn't copy the ${what}`),
  );
}

/** The selected client's one-click add, or why it can't be. */
function ClientAdd({ target }: { target: McpClientTarget | undefined }) {
  const registering = useMcpStore((s) => s.registering);
  const setRegistered = useMcpStore((s) => s.setRegistered);
  if (!target) return <span className="text-xs text-fg-3">Paste this into the client's MCP settings.</span>;
  const busy = registering === target.id;
  const run = (on: boolean) =>
    setRegistered(target.id, on).catch((e) => toast.error(`Couldn't update ${target.name}`, { description: String(e) }));
  if (target.registered) {
    return (
      <>
        <span className="mr-auto inline-flex min-w-0 items-center gap-[5px] text-xs text-fg-2">
          <Check className="size-3 shrink-0 text-ok" />
          <span className="truncate">Added to {target.name}{target.configPath ? ` · ${target.configPath}` : ""}</span>
        </span>
        <Button size="sm" variant="ghost" disabled={busy} onClick={() => void run(false)}>
          Remove
        </Button>
      </>
    );
  }
  if (!target.installed) {
    return <span className="mr-auto text-xs text-fg-3">{target.name} isn't on this computer; copy the config above instead.</span>;
  }
  return (
    <>
      <span className="mr-auto truncate text-xs text-fg-3">
        Writes the entry, token included, to {target.configPath ?? `${target.name}'s settings`}.
      </span>
      <Button size="sm" variant="primary" disabled={busy} onClick={() => void run(true)}>
        Add to {target.name}
      </Button>
    </>
  );
}

function ConnectClient({ url, token }: { url: string; token: string }) {
  const [client, setClient] = useState("claude");
  const [reveal, setReveal] = useState(false);
  const targets = useMcpStore((s) => s.targets);
  const loadTargets = useMcpStore((s) => s.loadTargets);
  useEffect(() => {
    void loadTargets().catch(() => {});
  }, [loadTargets]);

  async function regenerate() {
    try {
      await api.mcpRegenerateToken();
      useMcpStore.setState({ token: await api.mcpToken() });
      void loadTargets();
      toast("New token generated", { description: "Connected clients were disconnected; the ones added here were updated." });
    } catch (e) {
      toast.error("Couldn't generate a new token", { description: String(e) });
    }
  }

  return (
    <>
      <SectionTitle>Connect a client</SectionTitle>
      <div className="grid grid-cols-2 gap-3.5">
        <Field label="Endpoint">
          <div className="flex gap-1.5">
            <Input readOnly className="font-data" value={url} aria-label="Endpoint" />
            <Button size="icon" className="size-8" title="Copy" aria-label="Copy endpoint" onClick={() => copy(url, "endpoint")}>
              <Copy className="size-3.5" />
            </Button>
          </div>
        </Field>
        <Field
          label="Token"
          help={
            <>
              <Lock />
              Kept in the system keychain. Regenerating disconnects every client.
            </>
          }
        >
          <div className="flex gap-1.5">
            <Input readOnly className="font-data" value={reveal ? token : masked(token)} aria-label="Token" />
            <Button
              size="icon"
              className="size-8"
              title={reveal ? "Hide" : "Show"}
              aria-label={reveal ? "Hide token" : "Show token"}
              onClick={() => setReveal(!reveal)}
            >
              {reveal ? <EyeOff className="size-3.5" /> : <Eye className="size-3.5" />}
            </Button>
            <Button className="h-8" onClick={() => void regenerate()}>
              Regenerate
            </Button>
          </div>
        </Field>
      </div>
      <div className="overflow-hidden rounded-md border border-line bg-editor">
        <div className="flex h-9 items-center gap-2 border-b border-line-soft pr-1 pl-1">
          <SegmentedControl size="sm" aria-label="Client" value={client} onChange={setClient} options={SNIPPET_CLIENTS} />
          <span className="flex-1" />
          <Button
            variant="ghost"
            size="icon-sm"
            title="Copy"
            aria-label="Copy the config"
            onClick={() => copy(snippet(client, url, token), "config")}
          >
            <Copy />
          </Button>
        </div>
        <pre className="m-0 overflow-x-auto px-2.5 py-2 font-mono text-xs leading-[18px] whitespace-pre text-fg [font-variant-ligatures:none]">
          {snippet(client, url, reveal ? token : masked(token))
            .split("\n")
            .map((line, i) => (
              <div key={i} className={cn(/^\s*(#|\/\/)/.test(line) && "text-fg-3 italic")}>
                {line || " "}
              </div>
            ))}
        </pre>
        <div className="flex min-h-9 items-center gap-2 border-t border-line-soft py-1 pr-1.5 pl-2.5">
          <ClientAdd target={targets?.find((t) => t.id === client)} />
        </div>
      </div>
    </>
  );
}

/** Per-connection access in the sidebar's folders, a folder row setting all it holds. */
function ConnectionAccess() {
  const [query, setQuery] = useState("");
  const levels = useSettingsStore((s) => s.mcp.connections);
  const setLevels = useSettingsStore((s) => s.setMcpLevels);
  const { profiles, byId, shown } = useConnectionTree(query);
  const levelOf = (id: string): McpLevel => levels[id] ?? "none";
  const withAccess = profiles.filter((p) => levelOf(p.id) !== "none").length;

  const control = (ids: string[], label: string) => {
    const set = new Set(ids.map(levelOf));
    const value = set.size === 1 ? [...set][0] : ("mixed" as McpLevel);
    return (
      <SegmentedControl
        size="sm"
        aria-label={label}
        value={value}
        onChange={(level) => setLevels(ids, level)}
        options={LEVEL_OPTIONS}
      />
    );
  };

  return (
    <div className="flex flex-col gap-2">
      <ConnectionPickerHeader query={query} onQueryChange={setQuery} count={`${withAccess} of ${profiles.length} with access`} />
      <div className="rounded-md border border-line p-1">
        {shown.length === 0 ? (
          <ConnectionTreeEmpty hasProfiles={profiles.length > 0} query={query} />
        ) : (
          <ConnectionTree
            nodes={shown}
            byId={byId}
            rowsAreLabels={false}
            folderControl={() => null}
            folderTrailing={(folder, ids) => control(ids, `Access to everything in ${folder.name}`)}
            connectionTrailing={(p) => control([p.id], `Access to ${p.name}`)}
            aria-label="Connections clients may use"
          />
        )}
      </div>
    </div>
  );
}

/** "Always allow here" answers, each removable. */
function AlwaysAllowed() {
  const always = useSettingsStore((s) => s.mcp.always);
  const remove = useSettingsStore((s) => s.removeMcpAlways);
  const profiles = useConnectionsStore((s) => s.profiles);
  const rows = Object.entries(always).flatMap(([id, caps]) => caps.map((cap) => ({ id, cap })));
  if (rows.length === 0) return null;
  return (
    <>
      <SectionTitle>Always allowed</SectionTitle>
      <RowGroup>
        {rows.map(({ id, cap }) => {
          const p = profiles.find((x) => x.id === id);
          const any = id === "*";
          return (
            <GroupRow
              key={`${id}:${cap}`}
              className="py-1.5"
              leading={p && <ConnectionChip name={p.name} color={connectionColor(p.id, p.color)} size="sm" />}
              title={any ? "Any connection" : (p?.name ?? "A deleted connection")}
              description={`${CAP_LABELS[cap]}, without asking`}
            >
              <Button size="sm" variant="ghost" onClick={() => remove(id, cap)}>
                <X />
                Remove
              </Button>
            </GroupRow>
          );
        })}
      </RowGroup>
    </>
  );
}

function since(ms: number) {
  return new Date(ms).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
}

function Clients() {
  const clients = useMcpStore((s) => s.status?.clients ?? []);
  return (
    <>
      <SectionTitle>Clients</SectionTitle>
      {clients.length === 0 ? (
        <Note>No client has connected yet.</Note>
      ) : (
        <RowGroup>
          {clients.map((c) => {
            const idle = Date.now() - c.lastSeen > 5 * 60_000;
            return (
              <GroupRow
                key={c.id}
                className="py-[7px]"
                leading={<Bot className="size-3.5 shrink-0 text-fg-2" />}
                title={
                  <>
                    {c.name}
                    {c.version && <span className="ml-1 font-mono text-xs font-normal text-fg-3">{c.version}</span>}
                  </>
                }
                description={
                  idle ? (
                    `Idle since ${since(c.lastSeen)}`
                  ) : (
                    <>
                      <span className="mr-1.5 inline-block size-1.5 rounded-full bg-ok align-[1px]" />
                      Connected since {since(c.connectedAt)}
                    </>
                  )
                }
              >
                <Button size="sm" variant="ghost" onClick={() => void api.mcpDisconnectClient(c.id)}>
                  Disconnect
                </Button>
              </GroupRow>
            );
          })}
        </RowGroup>
      )}
    </>
  );
}

const RESULT_TONE = { ok: "text-ok", warn: "text-warn", err: "text-danger" } as const;

function Activity() {
  const activity = useMcpStore((s) => s.activity);
  const profiles = useConnectionsStore((s) => s.profiles);
  return (
    <>
      <SectionTitle>Recent activity</SectionTitle>
      {activity.length === 0 ? (
        <Note>Nothing yet. Every call a client makes shows here, allowed or not.</Note>
      ) : (
        <div className="rounded-md border border-line py-1 text-sm">
          {activity.map((a, i) => {
            const p = a.connectionId ? profiles.find((x) => x.id === a.connectionId) : undefined;
            return (
              <div key={i} className="grid h-[26px] grid-cols-[64px_88px_128px_minmax(0,1fr)_auto] items-center gap-2.5 px-3">
                <span className="font-mono text-[11px] text-fg-3">{new Date(a.at).toLocaleTimeString([], { hour12: false })}</span>
                <span className="truncate text-fg-2">{a.clientName}</span>
                <span className="truncate font-mono text-[11.5px] text-fg">{a.tool}</span>
                <span className="flex min-w-0 items-center gap-1.5 truncate text-fg-2">
                  {p && <span className="size-2 shrink-0 rounded-[2px]" style={{ background: connectionColor(p.id, p.color) }} />}
                  <span className="truncate">
                    <span className="font-mono text-[11.5px] text-fg">{a.target}</span>
                    {a.meta && <span className="text-fg-3"> · {a.meta}</span>}
                  </span>
                </span>
                <span className={cn("text-xs whitespace-nowrap", RESULT_TONE[a.result])}>{a.label}</span>
              </div>
            );
          })}
        </div>
      )}
    </>
  );
}

/** How Mongo Studio calls for attention when a request set to Ask arrives. */
function RequestAlerts() {
  const alerts = useSettingsStore((s) => s.mcpAlerts);
  const setAlerts = useSettingsStore((s) => s.setMcpAlerts);
  return (
    <div className="flex flex-col gap-3">
      <div className="flex items-start gap-2">
        <div className="min-w-0 flex-1">
          <SwitchField
            label="Play a sound"
            description="A short chime, so you hear it while you're in another window; the notification's sound when Mongo Studio is minimized."
            checked={alerts.sound}
            onCheckedChange={(sound) => setAlerts({ sound })}
          />
        </div>
        <Button variant="ghost" size="sm" onClick={() => void playChime(true)}>
          <Volume2 />
          Play it
        </Button>
      </div>
      <SwitchField
        label="Bring Mongo Studio to the front"
        description="Shows and focuses the window. Where the system won't let an app take the focus, like Linux on Wayland, a desktop notification opens it instead; elsewhere its taskbar entry flashes."
        checked={alerts.focus}
        onCheckedChange={(focus) => setAlerts({ focus })}
      />
    </div>
  );
}

export function McpSettings() {
  const mcp = useSettingsStore((s) => s.mcp);
  const setMcp = useSettingsStore((s) => s.setMcp);
  const setCap = useSettingsStore((s) => s.setMcpCap);
  const status = useMcpStore((s) => s.status);
  const token = useMcpStore((s) => s.token);
  const [port, setPort] = useState(String(mcp.port));
  const url = status?.url || `http://127.0.0.1:${mcp.port}/mcp`;

  useEffect(() => {
    if (mcp.enabled && !token) {
      api.mcpToken().then(
        (t) => useMcpStore.setState({ token: t }),
        () => {},
      );
    }
  }, [mcp.enabled, token]);

  function applyPort() {
    const n = Number(port);
    if (Number.isInteger(n) && n >= 1024 && n <= 65535) setMcp({ port: n });
    else setPort(String(mcp.port));
  }

  return (
    <>
      <SwitchField
        label="Run the MCP server"
        description="Lets AI clients on this computer, like Claude Code, Cursor and Codex, browse and change your databases through Mongo Studio, within the limits below. It runs while Mongo Studio is open."
        checked={mcp.enabled}
        onCheckedChange={(enabled) => setMcp({ enabled })}
      />
      {mcp.enabled && (
        <div className="-mt-1.5 ml-10 flex flex-wrap items-center gap-x-2 gap-y-1.5 text-sm text-fg-2">
          {status?.running ? (
            <>
              <span className="size-1.5 rounded-full bg-ok shadow-[0_0_0_2px_color-mix(in_srgb,var(--color-ok)_25%,transparent)]" />
              <span>
                Listening on <span className="font-mono text-[11.5px] text-fg">127.0.0.1:{status.port}</span>
              </span>
              <span className="text-fg-3">
                · local only · {status.clients.length} client{status.clients.length === 1 ? "" : "s"}
              </span>
            </>
          ) : status?.error ? (
            <span className="text-danger">{status.error}</span>
          ) : (
            <span className="text-fg-3">Starting…</span>
          )}
          <label className="ml-auto flex items-center gap-1.5 text-xs text-fg-3">
            Port
            <Input
              className="h-6 w-[72px] px-1.5 font-data text-xs"
              inputMode="numeric"
              value={port}
              onChange={(e) => setPort(e.target.value.replace(/\D/g, ""))}
              onBlur={applyPort}
              onKeyDown={(e) => e.key === "Enter" && applyPort()}
              aria-label="Port"
            />
            {mcp.port !== DEFAULT_MCP_PORT && (
              <button type="button" className="underline underline-offset-2 hover:text-fg" onClick={() => (setPort(String(DEFAULT_MCP_PORT)), setMcp({ port: DEFAULT_MCP_PORT }))}>
                Reset
              </button>
            )}
          </label>
        </div>
      )}

      <div className={cn("flex flex-col gap-4 transition-opacity duration-150", !mcp.enabled && "pointer-events-none opacity-45")}>
        <ConnectClient url={url} token={token ?? ""} />

        <SectionTitle>What clients can do</SectionTitle>
        <Note>
          Ask shows a request in Mongo Studio and waits for you. Write capabilities only reach connections set to Read
          &amp; write below.
        </Note>
        {CAP_GROUPS.map((group) => (
          <RowGroup key={group.title} title={group.title}>
            {group.rows.map((row) => {
              const locked = row.id === "browse";
              const policy: McpPolicy = locked ? "allow" : mcp.caps[row.id as CapId];
              return (
                <GroupRow key={row.id} title={row.label} description={row.help}>
                  <SegmentedControl
                    size="sm"
                    aria-label={row.label}
                    value={policy}
                    onChange={(p) => !locked && setCap(row.id as CapId, p)}
                    options={policyOptions(locked)}
                  />
                </GroupRow>
              );
            })}
          </RowGroup>
        ))}

        <SectionTitle>When a request waits for you</SectionTitle>
        <RequestAlerts />

        <SectionTitle>Connections clients may use</SectionTitle>
        <ConnectionAccess />
        <Note>New connections start at None. Connections a client creates start at Read.</Note>
        <AlwaysAllowed />

        <Aside icon={<Sparkles />}>
          The built-in Assistant doesn't use these grants: it stays read-only and reads only the connections allowed in{" "}
          <button
            type="button"
            className="text-accent-text underline underline-offset-2"
            onClick={() => useUiStore.getState().setSettingsSection("assistant")}
          >
            Assistant settings
          </button>
          .
        </Aside>

        <Clients />
        <Activity />
      </div>
    </>
  );
}
