import { useEffect, useRef, useState } from "react";
import type { ReactNode } from "react";
import { Check, Copy, SquareTerminal, Trash2 } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { useConsoleStore } from "../../store/consoleStore";
import { TabKind, useSessionsStore } from "../../store/sessionsStore";
import { cn } from "@/lib/utils";
import { useUiStore } from "../../store/uiStore";
import { buildEditScript } from "../../lib/editScript";
import { askDeleteDocuments, canDeleteFrom } from "../results/deleteDocuments";

interface DocumentActionsProps {
  doc: unknown;
  collectionName: string;
  /** The tab the document came from, whose console receives the edit. */
  tabId: string;
  /** Button size: the inspector header's, or a tree row's. */
  size?: "icon" | "icon-xs";
}

function Action({
  label,
  size,
  onClick,
  children,
  danger = false,
}: {
  label: string;
  size: "icon" | "icon-xs";
  onClick: () => void;
  children: ReactNode;
  /** Turns red on hover. */
  danger?: boolean;
}) {
  return (
    <Tooltip>
      <TooltipTrigger asChild>
        <Button
          variant="ghost"
          size={size}
          aria-label={label}
          className={cn(size === "icon" && "[&_svg]:size-3.5", danger && "hover:text-danger")}
          onClick={(e) => {
            e.stopPropagation();
            onClick();
          }}
        >
          {children}
        </Button>
      </TooltipTrigger>
      <TooltipContent>{label}</TooltipContent>
    </Tooltip>
  );
}

/** Copy-as-JSON, edit-in-console and delete, for the inspector, the inline document and the tree view. */
export function DocumentActions({ doc, collectionName, tabId, size = "icon" }: DocumentActionsProps) {
  const [copied, setCopied] = useState(false);
  const timer = useRef<ReturnType<typeof setTimeout>>(undefined);
  const deletable = useSessionsStore((s) => {
    const tab = s.tabs.find((t) => t.id === tabId);
    return tab?.kind === TabKind.Collection && canDeleteFrom(tab);
  });
  useEffect(() => () => clearTimeout(timer.current), []);

  async function handleCopy() {
    try {
      await navigator.clipboard.writeText(JSON.stringify(doc, null, 2));
      setCopied(true);
      clearTimeout(timer.current);
      timer.current = setTimeout(() => setCopied(false), 1200);
    } catch {
      // clipboard access denied - nothing more we can do here
    }
  }

  function handleEdit() {
    if (!doc || typeof doc !== "object") return;
    const script = buildEditScript(doc as Record<string, unknown>, collectionName);
    useConsoleStore.getState().setScript(tabId, script);
    useSessionsStore.getState().activateTab(tabId);
    useUiStore.getState().setMainTab(tabId, "console");
  }

  return (
    <>
      <Action label={copied ? "Copied" : "Copy as JSON"} size={size} onClick={handleCopy}>
        {copied ? <Check className="text-ok" /> : <Copy />}
      </Action>
      <Action label="Edit in console (updateOne)" size={size} onClick={handleEdit}>
        <SquareTerminal />
      </Action>
      {deletable && (
        <Action label="Delete document…" size={size} danger onClick={() => askDeleteDocuments(tabId, [doc])}>
          <Trash2 />
        </Action>
      )}
    </>
  );
}
