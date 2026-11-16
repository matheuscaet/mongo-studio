import { useEffect, type ReactNode } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { CircleAlert, Download, Lock, RefreshCw } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { cn } from "@/lib/utils";
import { formatMegabytes, useMongoshStore } from "../../store/mongoshStore";
import { useSettingsStore, type ScriptEngine } from "../../store/settingsStore";
import { Field } from "../connections/Field";
import { SectionTitle } from "../connections/SectionTitle";
import { SwitchField } from "../connections/SwitchField";
import { Aside, Lead } from "./parts";

interface EngineOptionProps {
  checked: boolean;
  disabled?: boolean;
  onSelect: () => void;
  name: string;
  version: ReactNode;
  children: ReactNode;
}

/** A radio card naming an engine, like the Assistant's agent choice. */
function EngineOption({ checked, disabled, onSelect, name, version, children }: EngineOptionProps) {
  return (
    <button
      type="button"
      role="radio"
      aria-checked={checked}
      disabled={disabled}
      onClick={onSelect}
      className={cn(
        "grid w-full grid-cols-[14px_1fr_auto] items-center gap-x-2.5 gap-y-[3px] rounded-md border border-line bg-editor px-2.5 py-[9px] text-left hover:border-field-line disabled:cursor-default disabled:opacity-70 disabled:hover:border-line",
        checked && "border-accent bg-[color-mix(in_srgb,var(--color-accent)_8%,var(--color-editor))] hover:border-accent",
      )}
    >
      <span className={cn("size-3.5 rounded-full border border-field-line bg-field", checked && "border-4 border-accent bg-white")} />
      <span className="font-medium">{name}</span>
      <span className="font-mono text-xs text-fg-3">{version}</span>
      {children}
    </button>
  );
}

const SOURCE_NOTE = {
  path: "Found on your PATH",
  downloaded: "Downloaded by Mongo Studio",
  custom: "The executable you picked",
} as const;

/** The download button, or the download's progress. */
function DownloadMongosh() {
  const info = useMongoshStore((s) => s.info);
  const download = useMongoshStore((s) => s.download);
  const error = useMongoshStore((s) => s.downloadError);
  const startDownload = useMongoshStore((s) => s.startDownload);

  if (download) {
    const pct = download.total ? Math.min(100, Math.round((download.received / download.total) * 100)) : null;
    const label =
      download.phase === "verify" ? "Checking the download…"
      : download.phase === "unpack" ? "Unpacking…"
      : download.phase === "done" ? "Done"
      : `Downloading mongosh… ${pct !== null ? `${pct}%` : formatMegabytes(download.received)}`;
    return (
      <div className="flex flex-col gap-1.5" role="status">
        <span className="text-sm text-fg-2">{label}</span>
        <div className="h-1 overflow-hidden rounded-full bg-fg/10">
          <div
            className="h-full rounded-full bg-accent transition-[width] duration-200"
            style={{ width: `${download.phase === "download" ? (pct ?? 5) : 100}%` }}
          />
        </div>
      </div>
    );
  }
  return (
    <div className="flex flex-col gap-1.5">
      <div className="flex items-center gap-3">
        <Button variant="primary" className="self-start" onClick={() => void startDownload()}>
          <Download className="size-3.5" />
          Download mongosh{info?.downloadSize ? ` (${formatMegabytes(info.downloadSize)})` : ""}
        </Button>
        <span className="text-xs text-fg-3">MongoDB's official build, checked and set up for you.</span>
      </div>
      {error && (
        <p role="alert" className="m-0 flex items-center gap-1.5 text-sm text-danger">
          <CircleAlert className="size-3.5 shrink-0" />
          {error}
        </p>
      )}
    </div>
  );
}

export function ConsoleSettings() {
  const engine = useSettingsStore((s) => s.consoleEngine);
  const setEngine = useSettingsStore((s) => s.setConsoleEngine);
  const mongosh = useSettingsStore((s) => s.mongosh);
  const setMongosh = useSettingsStore((s) => s.setMongosh);
  const info = useMongoshStore((s) => s.info);
  const detecting = useMongoshStore((s) => s.detecting);
  const detect = useMongoshStore((s) => s.detect);
  const found = Boolean(info?.path) && !info?.error;

  useEffect(() => {
    void detect();
  }, [detect, mongosh.path]);

  // Without mongosh, new consoles fall back to the built-in engine.
  const shown: ScriptEngine = found ? engine : "builtin";

  async function chooseExecutable() {
    const path = await open({ multiple: false, directory: false });
    if (typeof path === "string") setMongosh({ path });
  }

  return (
    <>
      <Lead>
        Runs console tabs and saved scripts. Each console can switch engine from its toolbar; this is what new consoles
        start with.
      </Lead>
      <SectionTitle>Engine</SectionTitle>
      <div role="radiogroup" aria-label="Script engine" className="flex flex-col gap-1.5">
        <EngineOption checked={shown === "builtin"} onSelect={() => setEngine("builtin")} name="Built-in" version="bundled">
          <span className="col-start-2 col-end-4 text-sm leading-[1.45] text-fg-3">
            Starts instantly, nothing to install. Most of mongosh's collection API with await (find, aggregate,
            insert, update, delete, distinct, bulkWrite); no shell helpers like show dbs or rs.status().
          </span>
        </EngineOption>
        <EngineOption
          checked={shown === "mongosh"}
          disabled={!found}
          onSelect={() => setEngine("mongosh")}
          name="mongosh"
          version={info?.version ?? ""}
        >
          <span className="col-start-2 col-end-4 font-mono text-[11px] leading-[1.4] text-fg-3 [font-variant-ligatures:none]">
            {info === null ? "Looking…" : (info.path ?? "Not found on this computer")}
          </span>
          <span className="col-start-2 col-end-4 inline-flex items-center gap-[5px] text-xs text-fg-2">
            {found && info?.source ? (
              <>
                <span className="size-1.5 shrink-0 rounded-full bg-ok" />
                <span>
                  {SOURCE_NOTE[info.source]} · the full MongoDB Shell, with its helpers. Write it as in mongosh: no{" "}
                  <code className="font-mono">await</code>, mongosh waits for you.
                </span>
              </>
            ) : info?.error && info.path ? (
              <>
                <CircleAlert className="size-3 shrink-0 text-danger" />
                {info.error}
              </>
            ) : info ? (
              "Download it below, or install it yourself and Rescan."
            ) : null}
          </span>
        </EngineOption>
      </div>
      <div className="flex items-center gap-2 text-xs text-fg-3">
        {detecting ? "Looking for mongosh…" : "Looked for mongosh on your PATH and in Mongo Studio's folder"}
        <Button size="sm" variant="ghost" className="ml-auto" disabled={detecting} onClick={() => void detect()}>
          <RefreshCw className={cn(detecting && "animate-spin")} />
          Rescan
        </Button>
      </div>
      {info !== null && !found && <DownloadMongosh />}

      <SectionTitle>mongosh</SectionTitle>
      <Field label="Executable" help={mongosh.path ? "Picked by you." : "Found automatically; pick another install to override it."}>
        <div className="flex gap-1.5">
          <Input
            readOnly
            className="font-data text-fg-2"
            value={mongosh.path ?? info?.path ?? ""}
            placeholder="Not found"
            aria-label="mongosh executable"
          />
          {mongosh.path && (
            <Button variant="ghost" onClick={() => setMongosh({ path: null })}>
              Use the one found
            </Button>
          )}
          <Button onClick={() => void chooseExecutable()}>Choose file…</Button>
        </div>
      </Field>
      <Field label="Extra arguments" htmlFor="settings-mongosh-args">
        <Input
          id="settings-mongosh-args"
          className="font-data"
          placeholder="e.g. --tls"
          value={mongosh.extraArgs}
          onChange={(e) => setMongosh({ extraArgs: e.target.value })}
        />
      </Field>
      <SwitchField
        label="Load ~/.mongoshrc.js"
        description="Your helpers and prompt tweaks. Turn off to keep runs reproducible."
        checked={mongosh.loadRc}
        onCheckedChange={(loadRc) => setMongosh({ loadRc })}
      />
      <Aside icon={<Lock />}>
        mongosh runs as a separate process per run, connected with that console's connection. The password goes in
        through an environment variable, <b>never the command line</b>. Results come back as Extended JSON, so Grid, Tree
        and JSON work as with the built-in engine.
      </Aside>
    </>
  );
}
