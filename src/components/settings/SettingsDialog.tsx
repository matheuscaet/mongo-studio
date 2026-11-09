import { useId, useRef, useState, type ReactNode } from "react";
import { CircleCheck, Palette, Plug, Search, Server, Sparkles, SquareTerminal, Table } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Dialog, DialogBody, DialogContent, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { SideNav } from "@/components/ui/side-nav";
import { AGENT_NAMES, useAssistantStore } from "../../store/assistantStore";
import { useMongoshStore } from "../../store/mongoshStore";
import { useSettingsStore } from "../../store/settingsStore";
import type { SettingsSection } from "../../store/uiStore";
import { ConsoleSettings } from "./ConsoleSettings";
import { McpSettings } from "./McpSettings";
import { AssistantSettings, ConnectionsSettings, GeneralSettings, ResultsSettings } from "./sections";

const SECTIONS: { id: SettingsSection; label: string; icon: ReactNode; keys: string; body: () => ReactNode }[] = [
  { id: "general", label: "General", icon: <Palette />, keys: "theme dark light system appearance", body: () => <GeneralSettings /> },
  { id: "results", label: "Results", icon: <Table />, keys: "view grid tree json default page limit layout", body: () => <ResultsSettings /> },
  { id: "console", label: "Console", icon: <SquareTerminal />, keys: "mongosh engine shell script download", body: () => <ConsoleSettings /> },
  { id: "connections", label: "Connections", icon: <Plug />, keys: "import export compass nosqlbooster", body: () => <ConnectionsSettings /> },
  { id: "assistant", label: "Assistant", icon: <Sparkles />, keys: "ai claude codex agent model", body: () => <AssistantSettings /> },
  { id: "mcp", label: "MCP server", icon: <Server />, keys: "mcp clients cursor claude codex vscode permissions token port sound notification alert focus request", body: () => <McpSettings /> },
];

interface SettingsDialogProps {
  section: SettingsSection;
  onSectionChange: (section: SettingsSection) => void;
  onClose: () => void;
}

/**
 * App settings in the connection dialog's shape: sections on the left with
 * a search, the section on the right. Every change applies and is saved as
 * it's made, so the dialog only has Close.
 */
export function SettingsDialog({ section, onSectionChange, onClose }: SettingsDialogProps) {
  const idPrefix = useId();
  const [query, setQuery] = useState("");
  const bodyRef = useRef<HTMLDivElement>(null);
  const engine = useSettingsStore((s) => s.consoleEngine);
  const mongoshFound = useMongoshStore((s) => Boolean(s.info?.path) && !s.info?.error);
  const mcpOn = useSettingsStore((s) => s.mcp.enabled);
  const assistantOn = useAssistantStore((s) => s.enabled);
  const agent = useAssistantStore((s) => s.agent);

  const states: Partial<Record<SettingsSection, string>> = {
    console: engine === "mongosh" && mongoshFound ? "mongosh" : "Built-in",
    assistant: assistantOn ? AGENT_NAMES[agent] : "Off",
    mcp: mcpOn ? "On" : "Off",
  };
  const q = query.trim().toLowerCase();
  const shown = SECTIONS.filter((s) => !q || `${s.label} ${s.keys}`.toLowerCase().includes(q));
  const current = SECTIONS.find((s) => s.id === section) ?? SECTIONS[0];

  function go(id: SettingsSection) {
    onSectionChange(id);
    bodyRef.current?.scrollTo(0, 0);
  }

  function search(value: string) {
    setQuery(value);
    const v = value.trim().toLowerCase();
    const matches = SECTIONS.filter((s) => `${s.label} ${s.keys}`.toLowerCase().includes(v));
    if (v && matches.length > 0 && !matches.some((s) => s.id === section)) go(matches[0].id);
  }

  return (
    <Dialog open onOpenChange={(open) => !open && onClose()}>
      <DialogContent
        aria-describedby={undefined}
        className="grid h-[min(680px,calc(100%-2rem))] w-[min(940px,calc(100%-2rem))] max-w-none grid-cols-[212px_minmax(0,1fr)] grid-rows-[auto_minmax(0,1fr)_auto] sm:max-w-none"
      >
        <DialogHeader className="col-span-full">
          <DialogTitle>Settings</DialogTitle>
        </DialogHeader>

        <SideNav
          aria-label="Settings sections"
          idPrefix={idPrefix}
          value={current.id}
          onChange={go}
          sections={shown.map((s) => ({ id: s.id, label: s.label, icon: s.icon, state: states[s.id] }))}
          header={
            <label className="mb-2 flex h-[30px] items-center gap-[7px] rounded-md border border-field-line bg-field px-2 text-fg-3 focus-within:border-accent focus-within:shadow-[0_0_0_2px_color-mix(in_srgb,var(--color-accent)_30%,transparent)]">
              <Search className="size-3.5 shrink-0" />
              <input
                className="min-w-0 flex-1 border-0 bg-transparent text-fg outline-none placeholder:text-fg-3"
                placeholder="Search settings"
                value={query}
                onChange={(e) => search(e.target.value)}
                aria-label="Search settings"
                autoComplete="off"
              />
            </label>
          }
          empty={<span className="px-2.5 py-1.5 text-sm text-fg-3">No setting matches “{query.trim()}”</span>}
        />

        <DialogBody
          ref={bodyRef}
          role="tabpanel"
          id={`${idPrefix}-panel`}
          aria-labelledby={`${idPrefix}-tab-${current.id}`}
          className="flex flex-col gap-4 pb-[22px]"
        >
          <h3 className="m-0 -mb-1 text-[14px] font-semibold">{current.label}</h3>
          {current.body()}
        </DialogBody>

        <DialogFooter className="col-span-full">
          <span className="flex flex-1 items-center gap-1.5 text-xs text-fg-3">
            <CircleCheck className="size-3 text-ok" />
            Saved as you change them
          </span>
          <Button onClick={onClose}>Close</Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
