import { useEffect } from "react";
import { Braces, Download, Info, ListTree, Monitor, Moon, PanelRight, Rows3, Sun, Table, Trash2, Upload } from "lucide-react";
import { Button } from "@/components/ui/button";
import { SegmentedControl } from "@/components/ui/segmented-control";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { cn } from "@/lib/utils";
import type { ThemePreference } from "@/lib/themes";
import { useAssistantStore } from "../../store/assistantStore";
import { PAGE_SIZES, useSettingsStore } from "../../store/settingsStore";
import { useThemeStore } from "../../store/themeStore";
import { useUiStore, type ConsoleLayout, type ResultView } from "../../store/uiStore";
import { AssistantSetupBody } from "../assistant/AssistantSetup";
import { Field } from "../connections/Field";
import { SwitchField } from "../connections/SwitchField";
import { Lead, SettingRow } from "./parts";

export function GeneralSettings() {
  const preference = useThemeStore((s) => s.preference);
  const setPreference = useThemeStore((s) => s.setPreference);
  const reopenTabs = useSettingsStore((s) => s.reopenTabs);
  const setReopenTabs = useSettingsStore((s) => s.setReopenTabs);
  return (
    <>
      <Field label="Theme">
        <SegmentedControl<ThemePreference>
          aria-label="Theme"
          className="self-start"
          value={preference}
          onChange={setPreference}
          options={[
            { value: "dark", label: "Dark", icon: <Moon /> },
            { value: "light", label: "Light", icon: <Sun /> },
            { value: "system", label: "Match system", icon: <Monitor /> },
          ]}
        />
      </Field>
      <SwitchField
        label="Reopen last session's tabs on launch"
        description="Connections reconnect when you open one of their tabs, not before."
        checked={reopenTabs}
        onCheckedChange={setReopenTabs}
      />
    </>
  );
}

export function ResultsSettings() {
  const defaultView = useUiStore((s) => s.defaultResultView);
  const setDefaultView = useUiStore((s) => s.setDefaultResultView);
  const layout = useUiStore((s) => s.consoleLayout);
  const setLayout = useUiStore((s) => s.setConsoleLayout);
  const pageSize = useSettingsStore((s) => s.pageSize);
  const setPageSize = useSettingsStore((s) => s.setPageSize);
  return (
    <>
      <Field
        label="Default view for new tabs"
        help={
          <>
            <Info />
            Every tab and every console keeps its own view. Switching one leaves the others as they are.
          </>
        }
      >
        <SegmentedControl<ResultView>
          aria-label="Default view"
          className="self-start"
          value={defaultView}
          onChange={setDefaultView}
          options={[
            { value: "grid", label: "Grid", icon: <Table /> },
            { value: "tree", label: "Tree", icon: <ListTree /> },
            { value: "json", label: "JSON", icon: <Braces /> },
          ]}
        />
      </Field>
      <div className="grid grid-cols-2 gap-3.5">
        <Field label="Documents per page" htmlFor="settings-page-size" help="Also the limit a new Find bar starts with.">
          <Select value={String(pageSize)} onValueChange={(v) => setPageSize(Number(v))}>
            <SelectTrigger id="settings-page-size" className="w-full">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              {PAGE_SIZES.map((n) => (
                <SelectItem key={n} value={String(n)}>
                  {n}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        </Field>
        <Field label="Console layout">
          <SegmentedControl<ConsoleLayout>
            aria-label="Console layout"
            className="self-start"
            value={layout}
            onChange={setLayout}
            options={[
              { value: "stacked", label: "Stacked", icon: <Rows3 /> },
              { value: "side", label: "Side by side", icon: <PanelRight /> },
            ]}
          />
        </Field>
      </div>
    </>
  );
}

export function ConnectionsSettings() {
  const openTransfer = useUiStore((s) => s.openTransfer);
  const openDeleteConnections = useUiStore((s) => s.openDeleteConnections);
  return (
    <>
      <Lead>Saved connections stay on this computer; their passwords stay in the system keychain.</Lead>
      <div>
        <SettingRow
          title="Import connections"
          description="From MongoDB Compass, NoSQLBooster or another Mongo Studio. Pick which ones before anything is saved."
        >
          <Button onClick={() => openTransfer("import")}>
            <Upload className="size-3.5" />
            Import…
          </Button>
        </SettingRow>
        <SettingRow
          title="Export connections"
          description="Choose which ones go in the file. Same JSON format Compass reads, so files round-trip between the two apps."
        >
          <Button onClick={() => openTransfer("export")}>
            <Download className="size-3.5" />
            Export…
          </Button>
        </SettingRow>
        <SettingRow
          title="Delete connections"
          description="Pick several saved connections and delete them at once, with their saved passwords."
        >
          <Button onClick={() => openDeleteConnections()}>
            <Trash2 className="size-3.5" />
            Delete…
          </Button>
        </SettingRow>
      </div>
    </>
  );
}

export function AssistantSettings() {
  const enabled = useAssistantStore((s) => s.enabled);
  const agent = useAssistantStore((s) => s.agent);
  const agents = useAssistantStore((s) => s.agents);
  useEffect(() => {
    if (useAssistantStore.getState().agents === null) void useAssistantStore.getState().detect();
  }, []);
  const found = agents?.some((a) => a.id === agent && a.path) ?? false;
  return (
    <>
      <SwitchField
        label="Turn on the Assistant"
        description={
          found || enabled
            ? "Adds the Assistant tool window (Ctrl L) and Ctrl I in the filter bar and the console."
            : "Install Claude Code or Codex and sign in once in a terminal, then Rescan below."
        }
        checked={enabled}
        disabled={!enabled && !found}
        onCheckedChange={(on) => (on ? useAssistantStore.getState().enable() : useAssistantStore.getState().disable())}
      />
      <div className={cn("flex flex-col gap-3.5", !enabled && "opacity-60")}>
        <AssistantSetupBody />
      </div>
    </>
  );
}
