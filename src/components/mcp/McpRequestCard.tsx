import { Bot, Check, CircleCheck, TriangleAlert } from "lucide-react";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import { ConnectionChip } from "@/components/ui/ConnectionChip";
import { EnvironmentTag } from "@/components/ui/EnvironmentTag";
import { connectionColor } from "@/lib/connectionColor";
import { useConnectionsStore } from "../../store/connectionsStore";
import { useMcpStore } from "../../store/mcpStore";
import type { McpCapability, McpRequestChoice } from "../../types/mcp";

const ASKING: Record<McpCapability, string> = {
  browse: "wants to browse",
  read: "wants to read",
  write: "wants to write",
  ddl: "wants to change the schema",
  script: "wants to run a script",
  connList: "wants to see your connections",
  connEdit: "wants to change a connection",
  connDelete: "wants to delete a connection",
  saveScript: "wants to save a script",
  relRead: "wants to read related documents",
  relEdit: "wants to propose a relation",
};

const POLICY_NAME: Record<McpCapability, string> = {
  browse: "Browsing",
  read: "Reading documents",
  write: "Insert, update and delete",
  ddl: "Collections and indexes",
  script: "Running scripts",
  connList: "Seeing connections",
  connEdit: "Editing connections",
  connDelete: "Deleting connections",
  saveScript: "Saving scripts",
  relRead: "Reading relations",
  relEdit: "Proposing relations",
};

/**
 * A call an MCP client made that's set to Ask, over the workspace's
 * bottom-right corner until answered: what it would do, where, and a dry-run
 * count. One at a time, oldest first; the status bar counts the rest.
 */
export function McpRequestCard() {
  const request = useMcpStore((s) => s.requests[0]);
  const queued = useMcpStore((s) => s.requests.length - 1);
  const answer = useMcpStore((s) => s.answer);
  const profile = useConnectionsStore((s) => s.profiles.find((p) => p.id === request?.connectionId));
  if (!request) return null;

  const respond = (choice: McpRequestChoice) =>
    answer(request.requestId, choice).catch((e) =>
      toast.error("Couldn't answer the request", { description: String(e) }),
    );
  const time = new Date(request.at).toLocaleTimeString([], { hour12: false });

  return (
    <div
      role="alertdialog"
      aria-labelledby="mcp-request-client"
      className="fixed right-4 bottom-10 z-40 w-[min(540px,calc(100%-2rem))] overflow-hidden rounded-md border border-line bg-popover shadow-overlay animate-in fade-in-0 slide-in-from-bottom-1 duration-200"
    >
      <div className="flex h-8 items-center gap-[7px] border-b border-line-soft pr-2.5 pl-2.5 text-sm text-fg-2">
        <Bot className="size-3.5 shrink-0" />
        <b id="mcp-request-client" className="font-semibold text-fg">
          {request.clientName}
        </b>
        <span className="min-w-0 flex-1 truncate text-fg-3">{ASKING[request.capability] ?? "is asking"} · via MCP</span>
        {queued > 0 && <span className="text-xs text-warn">+{queued} more</span>}
        <span className="font-mono text-[11px] text-fg-3">{time}</span>
      </div>
      <div className="flex items-start gap-2 border-b border-line-soft bg-warn/9 px-2.5 py-2 text-sm leading-[1.45] text-fg-2">
        <TriangleAlert className="mt-px size-3.5 shrink-0 text-warn" />
        <span>
          <b className="font-medium text-fg">{request.title}</b>
          {request.connectionName && " on "}
          {profile && (
            <span className="mx-0.5 inline-block">
              <ConnectionChip name={profile.name} color={connectionColor(profile.id, profile.color)} size="dot" />
            </span>
          )}
          {request.connectionName}.{" "}
          {request.production ? (
            <>
              <EnvironmentTag environment="production" className="mx-0.5 align-[1px]" /> Changes on production are
              always asked about.
            </>
          ) : (
            <>{POLICY_NAME[request.capability]} is set to Ask.</>
          )}
        </span>
      </div>
      <pre className="m-0 max-h-[220px] overflow-auto border-b border-line-soft bg-editor px-2.5 py-2 font-mono text-xs leading-[18px] whitespace-pre text-fg [font-variant-ligatures:none]">
        {request.code}
      </pre>
      <div className="flex flex-wrap items-center gap-1 py-1.5 pr-1.5 pl-2.5">
        <span className="mr-auto inline-flex items-center gap-[5px] text-xs whitespace-nowrap text-fg-2">
          {request.dryRun && (
            <>
              <CircleCheck className="size-3 text-ok" />
              Dry run: {request.dryRun}
            </>
          )}
        </span>
        <Button size="sm" variant="ghost" onClick={() => void respond("deny")}>
          Deny
        </Button>
        {!request.production && (
          <Button
            size="sm"
            title={`Set ${POLICY_NAME[request.capability].toLowerCase()}${request.connectionName ? ` on ${request.connectionName}` : ""} to Allow`}
            onClick={() => void respond("always")}
            aria-label={request.connectionId ? "Always allow here" : "Always allow"}
          >
            {request.connectionId ? "Always allow here" : "Always allow"}
          </Button>
        )}
        <Button size="sm" variant="primary" onClick={() => void respond("once")}>
          <Check />
          Allow once
        </Button>
      </div>
    </div>
  );
}
