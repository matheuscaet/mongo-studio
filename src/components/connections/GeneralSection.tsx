import { Lock } from "lucide-react";
import { Checkbox } from "@/components/ui/checkbox";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { SegmentedControl } from "@/components/ui/segmented-control";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Textarea } from "@/components/ui/textarea";
import { useConnectionsStore } from "@/store/connectionsStore";
import { cn } from "@/lib/utils";
import { ENVIRONMENT_LABELS, type ConnectionEnvironment, type ConnectionSource } from "@/types/connection";
import { ColorSwatches } from "./ColorSwatches";
import { DATA_FIELD, KEEP_SAVED, type ConnectionSectionProps } from "./connectionFormTypes";
import { useFolderOptions } from "./connectionFolder";
import { Field } from "./Field";
import { FieldRow } from "./FieldRow";
import { ParsedUriChips } from "./ParsedUriChips";
import { SectionTitle } from "./SectionTitle";

const NO_FOLDER = "__root";
const NO_ENVIRONMENT = "__none";

interface GeneralSectionProps extends ConnectionSectionProps {
  folder: string | null;
  onFolderChange: (folderId: string | null) => void;
  /** The color automatic resolves to for this connection. */
  autoColor: string;
}

export function GeneralSection({
  input,
  update,
  editing,
  folder,
  onFolderChange,
  autoColor,
}: GeneralSectionProps) {
  const folders = useFolderOptions();
  const keychain = useConnectionsStore((s) => s.secretBackend?.backend !== "encrypted_file");
  const source = input.source;

  return (
    <>
      <FieldRow>
        <Field label="Name" htmlFor="conn-name">
          <Input
            id="conn-name"
            value={input.name}
            onChange={(e) => update("name", e.target.value)}
            placeholder="e.g. Staging EU"
          />
        </Field>
        {/* folders live in the sidebar layout, not the profile */}
        <Field label="Folder" htmlFor="conn-folder">
          <Select
            value={folder ?? NO_FOLDER}
            onValueChange={(v) => onFolderChange(v === NO_FOLDER ? null : v)}
          >
            <SelectTrigger id="conn-folder" className="w-full">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value={NO_FOLDER}>No folder</SelectItem>
              {folders.map((f) => (
                <SelectItem key={f.id} value={f.id}>
                  {f.label}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        </Field>
      </FieldRow>

      <Field
        label="Color"
        help="Marks this server's tabs, toolbar and status bar, so production never looks like local."
      >
        <ColorSwatches
          value={input.color ?? null}
          onChange={(color) => update("color", color)}
          autoColor={autoColor}
        />
      </Field>

      <Field
        label="Environment"
        htmlFor="conn-environment"
        help="Shown next to its name. On Production, MCP clients have to ask before every change."
      >
        <Select
          value={input.environment ?? NO_ENVIRONMENT}
          onValueChange={(v) =>
            update("environment", v === NO_ENVIRONMENT ? null : (v as ConnectionEnvironment))
          }
        >
          <SelectTrigger id="conn-environment" className="w-full sm:w-[220px]">
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            <SelectItem value={NO_ENVIRONMENT}>Not set</SelectItem>
            {(Object.keys(ENVIRONMENT_LABELS) as ConnectionEnvironment[]).map((env) => (
              <SelectItem key={env} value={env}>
                {ENVIRONMENT_LABELS[env].name}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
      </Field>

      <Field label="Connect with">
        <SegmentedControl<ConnectionSource["kind"]>
          aria-label="Connect with"
          value={source.kind}
          onChange={(kind) =>
            update(
              "source",
              kind === "uri"
                ? { kind: "uri", uri: "" }
                : { kind: "manual", host: "", port: 27017, srv: false },
            )
          }
          options={[
            { value: "uri", label: "Connection string" },
            { value: "manual", label: "Host and port" },
          ]}
        />
      </Field>

      {source.kind === "uri" ? (
        <Field label="Connection string" htmlFor="conn-uri">
          <Textarea
            id="conn-uri"
            className={cn(DATA_FIELD, "h-16 field-sizing-fixed")}
            spellCheck={false}
            autoCapitalize="off"
            autoCorrect="off"
            value={source.uri}
            onChange={(e) => update("source", { kind: "uri", uri: e.target.value })}
            placeholder="mongodb+srv://user:pass@cluster0.example.mongodb.net/mydb"
          />
          <ParsedUriChips uri={source.uri} />
        </Field>
      ) : (
        <>
          <FieldRow>
            <Field label="Host" htmlFor="conn-host">
              <Input
                id="conn-host"
                className={DATA_FIELD}
                spellCheck={false}
                value={source.host}
                onChange={(e) => update("source", { ...source, host: e.target.value })}
                placeholder="localhost"
              />
            </Field>
            <Field label="Port" htmlFor="conn-port">
              <Input
                id="conn-port"
                className={DATA_FIELD}
                type="number"
                value={source.port}
                disabled={source.srv}
                onChange={(e) => update("source", { ...source, port: Number(e.target.value) })}
              />
            </Field>
          </FieldRow>
          <Label className="gap-2 font-normal">
            <Checkbox
              checked={source.srv}
              onCheckedChange={(checked) => update("source", { ...source, srv: checked === true })}
            />
            Use SRV record (mongodb+srv://)
          </Label>
        </>
      )}

      <SectionTitle>Authentication</SectionTitle>
      <FieldRow>
        <Field label="Username" htmlFor="conn-user">
          <Input
            id="conn-user"
            autoComplete="off"
            value={input.username ?? ""}
            onChange={(e) => update("username", e.target.value || null)}
            placeholder={source.kind === "uri" ? "From the connection string" : "Optional"}
          />
        </Field>
        <Field
          label="Password"
          htmlFor="conn-password"
          help={
            <>
              <Lock />
              {keychain
                ? "Saved in the system keychain, never in the connection file"
                : "Saved encrypted, never in the connection file"}
            </>
          }
        >
          <Input
            id="conn-password"
            type="password"
            autoComplete="new-password"
            value={input.password ?? ""}
            onChange={(e) => update("password", e.target.value || null)}
            placeholder={editing?.hasPassword ? KEEP_SAVED : "Optional"}
          />
        </Field>
      </FieldRow>
    </>
  );
}
