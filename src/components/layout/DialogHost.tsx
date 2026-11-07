import { lazy, Suspense } from "react";
import { useUiStore } from "../../store/uiStore";
import { ConfirmDialog } from "../common/ConfirmDialog";
import { DeleteDocumentsDialog } from "../results/DeleteDocumentsDialog";

// Loaded on first use, keeping them out of the startup bundle.
const ConnectionForm = lazy(() =>
  import("../connections/ConnectionForm").then((m) => ({ default: m.ConnectionForm })),
);
const ConnectionsImportExportDialog = lazy(() =>
  import("../connections/ConnectionsImportExportDialog").then((m) => ({
    default: m.ConnectionsImportExportDialog,
  })),
);
const SettingsDialog = lazy(() =>
  import("../settings/SettingsDialog").then((m) => ({ default: m.SettingsDialog })),
);
const EditConnectionDialog = lazy(() =>
  import("./EditConnectionDialog").then((m) => ({ default: m.EditConnectionDialog })),
);

/**
 * App-level dialogs, opened from anywhere through uiStore: the connection
 * form (for a new connection or a saved one), import / export and
 * Settings; and the
 * questions `confirm()` asks, above them.
 */
export function DialogHost() {
  const dialog = useUiStore((s) => s.connectionDialog);
  const setConnectionDialog = useUiStore((s) => s.setConnectionDialog);
  const transfer = useUiStore((s) => s.transfer);
  const closeTransfer = useUiStore((s) => s.closeTransfer);
  const settingsSection = useUiStore((s) => s.settingsSection);
  const setSettingsSection = useUiStore((s) => s.setSettingsSection);
  const close = () => setConnectionDialog(null);

  let content = null;
  if (dialog?.mode === "new") content = <ConnectionForm onSaved={close} onCancel={close} />;
  else if (dialog?.mode === "edit") {
    content = <EditConnectionDialog key={dialog.id} id={dialog.id} onClose={close} />;
  } else if (transfer) {
    content = <ConnectionsImportExportDialog initial={transfer} onClose={closeTransfer} />;
  } else if (settingsSection) {
    content = (
      <SettingsDialog
        section={settingsSection}
        onSectionChange={setSettingsSection}
        onClose={() => setSettingsSection(null)}
      />
    );
  }
  return (
    <>
      <Suspense fallback={null}>{content}</Suspense>
      <DeleteDocumentsDialog />
      <ConfirmDialog />
    </>
  );
}
