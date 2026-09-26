import { useEffect, useState } from "react";
import { useConnectionsStore } from "../../store/connectionsStore";
import { Modal } from "../ui/Modal";
import {
  emptyAdvancedOptions,
  emptySshTunnelOptions,
  newProfileInput,
  profileToInput,
  type ConnectionProfile,
  type ConnectionProfileInput,
} from "../../types/connection";

const inputClass =
  "w-full rounded border border-border-subtle bg-panel px-2 py-1 text-sm text-text-default placeholder:text-text-faint focus:border-accent focus:outline-none";
const labelClass = "block text-xs text-text-muted mb-1";
const sectionClass = "rounded border border-border-subtle p-3";

type ConnectionTab = "general" | "tls" | "ssh" | "advanced";

const tabs: { id: ConnectionTab; label: string }[] = [
  { id: "general", label: "General" },
  { id: "tls", label: "TLS" },
  { id: "ssh", label: "SSH tunnel" },
  { id: "advanced", label: "Advanced" },
];

interface ConnectionFormProps {
  /** A saved profile to edit; a new connection when absent. */
  editing?: ConnectionProfile;
  onSaved: () => void;
  onCancel: () => void;
}

/** Placeholder for a secret field when editing: blank keeps the saved one. */
const KEEP_SAVED = "Saved - leave blank to keep";

export function ConnectionForm({ editing, onSaved, onCancel }: ConnectionFormProps) {
  const [input, setInput] = useState<ConnectionProfileInput>(() =>
    editing ? profileToInput(editing) : newProfileInput(),
  );
  const editingActive = useConnectionsStore(
    (s) => editing !== undefined && s.sessions[editing.id] !== undefined,
  );
  const [tab, setTab] = useState<ConnectionTab>("general");
  const { saveProfile, testConnection, lastTestResult, loading, error } =
    useConnectionsStore();

  const sourceKind = input.source.kind;

  // Tabs hide their fields, so mark the ones holding a non-default setting.
  const tabHasSettings: Record<ConnectionTab, boolean> = {
    general: false,
    tls:
      input.tls.enabled ||
      input.tls.allowInvalidCertificates ||
      Boolean(input.tls.caFile || input.tls.certKeyFile || input.tlsCertKeyPassphrase),
    ssh: Boolean(input.sshTunnel?.enabled),
    advanced: Object.values(input.advanced).some((v) => v !== null),
  };

  function update<K extends keyof ConnectionProfileInput>(
    key: K,
    value: ConnectionProfileInput[K],
  ) {
    setInput((prev) => ({ ...prev, [key]: value }));
  }

  // Test result and error live in the store, so clear whatever a previous
  // visit left behind instead of reopening the dialog with a stale banner.
  useEffect(() => {
    useConnectionsStore.setState({ lastTestResult: null, error: null });
  }, []);

  async function handleSave() {
    try {
      await saveProfile(input);
      onSaved();
    } catch {
      // saveProfile already put the message in the store; keep the dialog open
    }
  }

  return (
    <Modal
      title={editing ? `Edit connection - ${editing.name}` : "New connection"}
      width="max-w-2xl"
      onClose={onCancel}
      subheader={
        <div className="flex gap-1 px-3">
          {tabs.map((t) => (
            <button
              key={t.id}
              type="button"
              className={`flex items-center gap-1.5 border-b-2 px-3 py-2 text-xs ${
                tab === t.id
                  ? "border-accent text-text-default"
                  : "border-transparent text-text-muted hover:text-text-default"
              }`}
              onClick={() => setTab(t.id)}
            >
              {t.label}
              {tabHasSettings[t.id] && (
                <span
                  className="h-1.5 w-1.5 rounded-full bg-status-green"
                  title="Configured"
                />
              )}
            </button>
          ))}
        </div>
      }
      footer={
        <div className="flex flex-col gap-2">
          {editingActive && (
            <div className="rounded bg-panel-alt p-2 text-xs text-text-muted">
              You're connected with this connection. Changes apply the next time you
              connect.
            </div>
          )}
          {lastTestResult && (
            <div
              className={`rounded p-2 text-xs ${
                lastTestResult.success
                  ? "bg-emerald-950 text-emerald-300"
                  : "bg-red-950 text-red-300"
              }`}
            >
              {lastTestResult.message}
              {lastTestResult.serverVersion &&
                ` (MongoDB ${lastTestResult.serverVersion})`}
            </div>
          )}
          {error && (
            <div className="rounded bg-red-950 p-2 text-xs text-red-300">{error}</div>
          )}
          <div className="flex justify-end gap-2">
            <button
              type="button"
              className="rounded bg-panel-alt px-3 py-1.5 text-xs text-text-default hover:bg-panel-hover"
              onClick={onCancel}
            >
              Cancel
            </button>
            <button
              type="button"
              disabled={loading}
              className="rounded bg-panel-alt px-3 py-1.5 text-xs text-text-default hover:bg-panel-hover disabled:opacity-50"
              onClick={() => testConnection(input)}
            >
              Test connection
            </button>
            <button
              type="button"
              disabled={loading || !input.name}
              className="rounded bg-run px-3 py-1.5 text-xs text-white hover:bg-run-hover disabled:opacity-50"
              onClick={handleSave}
            >
              Save
            </button>
          </div>
        </div>
      }
    >
      <div className="flex min-h-[19rem] flex-col gap-4 text-text-default">
        {tab === "general" && (
          <>
            <div>
              <label className={labelClass}>Name</label>
              <input
                className={inputClass}
                autoFocus
                value={input.name}
                onChange={(e) => update("name", e.target.value)}
                placeholder="My cluster"
              />
            </div>

            <div className={sectionClass}>
              <div className="mb-2 flex gap-3 text-xs">
                <label className="flex items-center gap-1">
                  <input
                    type="radio"
                    checked={sourceKind === "uri"}
                    onChange={() => update("source", { kind: "uri", uri: "" })}
                  />
                  Connection string
                </label>
                <label className="flex items-center gap-1">
                  <input
                    type="radio"
                    checked={sourceKind === "manual"}
                    onChange={() =>
                      update("source", { kind: "manual", host: "", port: 27017, srv: false })
                    }
                  />
                  Host / port
                </label>
              </div>

              {input.source.kind === "uri" ? (
                <div>
                  <label className={labelClass}>
                    URI (mongodb:// or mongodb+srv://)
                  </label>
                  <input
                    className={inputClass}
                    value={input.source.uri}
                    onChange={(e) =>
                      update("source", { kind: "uri", uri: e.target.value })
                    }
                    placeholder="mongodb+srv://user:pass@cluster0.example.mongodb.net/mydb"
                  />
                </div>
              ) : (
                <div className="grid grid-cols-3 gap-2">
                  <div className="col-span-2">
                    <label className={labelClass}>Host</label>
                    <input
                      className={inputClass}
                      value={input.source.host}
                      onChange={(e) =>
                        update("source", { ...input.source, host: e.target.value } as ConnectionProfileInput["source"])
                      }
                    />
                  </div>
                  <div>
                    <label className={labelClass}>Port</label>
                    <input
                      className={inputClass}
                      type="number"
                      value={input.source.port}
                      onChange={(e) =>
                        update("source", { ...input.source, port: Number(e.target.value) } as ConnectionProfileInput["source"])
                      }
                    />
                  </div>
                  <label className="col-span-3 flex items-center gap-1 text-xs">
                    <input
                      type="checkbox"
                      checked={input.source.srv}
                      onChange={(e) =>
                        update("source", { ...input.source, srv: e.target.checked } as ConnectionProfileInput["source"])
                      }
                    />
                    Use SRV record (mongodb+srv://)
                  </label>
                </div>
              )}
            </div>

            <div className={`${sectionClass} grid grid-cols-2 gap-2`}>
              <div>
                <label className={labelClass}>Username (optional override)</label>
                <input
                  className={inputClass}
                  value={input.username ?? ""}
                  onChange={(e) => update("username", e.target.value || null)}
                />
              </div>
              <div>
                <label className={labelClass}>Password (optional override)</label>
                <input
                  className={inputClass}
                  type="password"
                  value={input.password ?? ""}
                  onChange={(e) => update("password", e.target.value || null)}
                  placeholder={editing?.hasPassword ? KEEP_SAVED : undefined}
                />
              </div>
            </div>
          </>
        )}

        {tab === "tls" && (
          <div className="flex flex-col gap-2">
            <label className="flex items-center gap-1 text-xs">
              <input
                type="checkbox"
                checked={input.tls.enabled}
                onChange={(e) =>
                  update("tls", { ...input.tls, enabled: e.target.checked })
                }
              />
              Enable TLS
            </label>
            <div>
              <label className={labelClass}>CA file path</label>
              <input
                className={inputClass}
                value={input.tls.caFile ?? ""}
                onChange={(e) =>
                  update("tls", { ...input.tls, caFile: e.target.value || null })
                }
              />
            </div>
            <div>
              <label className={labelClass}>Client cert/key file path</label>
              <input
                className={inputClass}
                value={input.tls.certKeyFile ?? ""}
                onChange={(e) =>
                  update("tls", { ...input.tls, certKeyFile: e.target.value || null })
                }
              />
            </div>
            <div>
              <label className={labelClass}>Client cert/key passphrase</label>
              <input
                className={inputClass}
                type="password"
                value={input.tlsCertKeyPassphrase ?? ""}
                onChange={(e) => update("tlsCertKeyPassphrase", e.target.value || null)}
                placeholder={editing?.tls.certKeyHasPassphrase ? KEEP_SAVED : undefined}
              />
            </div>
            <label className="flex items-center gap-1 text-xs">
              <input
                type="checkbox"
                checked={input.tls.allowInvalidCertificates}
                onChange={(e) =>
                  update("tls", { ...input.tls, allowInvalidCertificates: e.target.checked })
                }
              />
              Allow invalid certificates (testing only)
            </label>
          </div>
        )}

        {tab === "ssh" && (
          <div className="flex flex-col gap-2">
            <label className="flex items-center gap-1 text-xs">
              <input
                type="checkbox"
                checked={input.sshTunnel?.enabled ?? false}
                onChange={(e) =>
                  update(
                    "sshTunnel",
                    e.target.checked
                      ? { ...emptySshTunnelOptions(), enabled: true }
                      : null,
                  )
                }
              />
              Tunnel through SSH
            </label>
            {input.sshTunnel && (
              <>
                <div className="grid grid-cols-3 gap-2">
                  <div className="col-span-2">
                    <label className={labelClass}>SSH host</label>
                    <input
                      className={inputClass}
                      value={input.sshTunnel.host}
                      onChange={(e) =>
                        update("sshTunnel", { ...input.sshTunnel!, host: e.target.value })
                      }
                    />
                  </div>
                  <div>
                    <label className={labelClass}>Port</label>
                    <input
                      className={inputClass}
                      type="number"
                      value={input.sshTunnel.port}
                      onChange={(e) =>
                        update("sshTunnel", { ...input.sshTunnel!, port: Number(e.target.value) })
                      }
                    />
                  </div>
                </div>
                <div>
                  <label className={labelClass}>SSH username</label>
                  <input
                    className={inputClass}
                    value={input.sshTunnel.username}
                    onChange={(e) =>
                      update("sshTunnel", { ...input.sshTunnel!, username: e.target.value })
                    }
                  />
                </div>
                <div>
                  <label className={labelClass}>Auth method</label>
                  <select
                    className={inputClass}
                    value={input.sshTunnel.authMethod}
                    onChange={(e) =>
                      update("sshTunnel", {
                        ...input.sshTunnel!,
                        authMethod: e.target.value as "password" | "private_key" | "agent",
                      })
                    }
                  >
                    <option value="password">Password</option>
                    <option value="private_key">Private key</option>
                    <option value="agent">SSH agent (not yet supported)</option>
                  </select>
                </div>
                {input.sshTunnel.authMethod === "password" && (
                  <div>
                    <label className={labelClass}>SSH password</label>
                    <input
                      className={inputClass}
                      type="password"
                      value={input.sshPassword ?? ""}
                      onChange={(e) => update("sshPassword", e.target.value || null)}
                      placeholder={editing?.sshTunnel ? KEEP_SAVED : undefined}
                    />
                  </div>
                )}
                {input.sshTunnel.authMethod === "private_key" && (
                  <>
                    <div>
                      <label className={labelClass}>Private key path</label>
                      <input
                        className={inputClass}
                        value={input.sshTunnel.privateKeyPath ?? ""}
                        onChange={(e) =>
                          update("sshTunnel", {
                            ...input.sshTunnel!,
                            privateKeyPath: e.target.value || null,
                          })
                        }
                      />
                    </div>
                    <div>
                      <label className={labelClass}>Key passphrase (if encrypted)</label>
                      <input
                        className={inputClass}
                        type="password"
                        value={input.sshKeyPassphrase ?? ""}
                        onChange={(e) => update("sshKeyPassphrase", e.target.value || null)}
                        placeholder={
                          editing?.sshTunnel?.privateKeyHasPassphrase ? KEEP_SAVED : undefined
                        }
                      />
                    </div>
                  </>
                )}
              </>
            )}
          </div>
        )}

        {tab === "advanced" && (
          <div>
            <div className="grid grid-cols-2 gap-2">
              <div>
                <label className={labelClass}>App name</label>
                <input
                  className={inputClass}
                  value={input.advanced.appName ?? ""}
                  onChange={(e) =>
                    update("advanced", { ...input.advanced, appName: e.target.value || null })
                  }
                />
              </div>
              <div>
                <label className={labelClass}>Replica set</label>
                <input
                  className={inputClass}
                  value={input.advanced.replicaSet ?? ""}
                  onChange={(e) =>
                    update("advanced", { ...input.advanced, replicaSet: e.target.value || null })
                  }
                />
              </div>
              <div>
                <label className={labelClass}>Connect timeout (ms)</label>
                <input
                  className={inputClass}
                  type="number"
                  value={input.advanced.connectTimeoutMs ?? ""}
                  onChange={(e) =>
                    update("advanced", {
                      ...input.advanced,
                      connectTimeoutMs: e.target.value ? Number(e.target.value) : null,
                    })
                  }
                />
              </div>
              <div>
                <label className={labelClass}>Server selection timeout (ms)</label>
                <input
                  className={inputClass}
                  type="number"
                  value={input.advanced.serverSelectionTimeoutMs ?? ""}
                  onChange={(e) =>
                    update("advanced", {
                      ...input.advanced,
                      serverSelectionTimeoutMs: e.target.value ? Number(e.target.value) : null,
                    })
                  }
                />
              </div>
              <div>
                <label className={labelClass}>Max pool size</label>
                <input
                  className={inputClass}
                  type="number"
                  value={input.advanced.maxPoolSize ?? ""}
                  onChange={(e) =>
                    update("advanced", {
                      ...input.advanced,
                      maxPoolSize: e.target.value ? Number(e.target.value) : null,
                    })
                  }
                />
              </div>
              <div>
                <label className={labelClass}>Min pool size</label>
                <input
                  className={inputClass}
                  type="number"
                  value={input.advanced.minPoolSize ?? ""}
                  onChange={(e) =>
                    update("advanced", {
                      ...input.advanced,
                      minPoolSize: e.target.value ? Number(e.target.value) : null,
                    })
                  }
                />
              </div>
              <div>
                <label className={labelClass}>Auth mechanism</label>
                <select
                  className={inputClass}
                  value={input.advanced.authMechanism ?? ""}
                  onChange={(e) =>
                    update("advanced", {
                      ...input.advanced,
                      authMechanism: (e.target.value || null) as ConnectionProfileInput["advanced"]["authMechanism"],
                    })
                  }
                >
                  <option value="">Negotiate automatically</option>
                  <option value="SCRAM_SHA1">SCRAM-SHA-1</option>
                  <option value="SCRAM_SHA256">SCRAM-SHA-256</option>
                  <option value="MONGODB_X509">X.509</option>
                  <option value="MONGODB_AWS">MONGODB-AWS (IAM)</option>
                  <option value="GSSAPI">Kerberos (GSSAPI, requires special build)</option>
                </select>
              </div>
              <div>
                <label className={labelClass}>Auth source</label>
                <input
                  className={inputClass}
                  value={input.advanced.authSource ?? ""}
                  onChange={(e) =>
                    update("advanced", { ...input.advanced, authSource: e.target.value || null })
                  }
                />
              </div>
              <label className="flex items-center gap-1 text-xs">
                <input
                  type="checkbox"
                  checked={input.advanced.retryWrites ?? true}
                  onChange={(e) =>
                    update("advanced", { ...input.advanced, retryWrites: e.target.checked })
                  }
                />
                Retry writes
              </label>
              <label className="flex items-center gap-1 text-xs">
                <input
                  type="checkbox"
                  checked={input.advanced.directConnection ?? false}
                  onChange={(e) =>
                    update("advanced", { ...input.advanced, directConnection: e.target.checked })
                  }
                />
                Direct connection (skip topology discovery)
              </label>
            </div>
            <button
              type="button"
              className="mt-2 text-xs text-text-muted underline"
              onClick={() => update("advanced", emptyAdvancedOptions())}
            >
              Reset advanced options
            </button>
          </div>
        )}
      </div>
    </Modal>
  );
}
