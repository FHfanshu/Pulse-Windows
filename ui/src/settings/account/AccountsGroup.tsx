// Ported from upstream Settings/AccountsGroup.swift: signing in to another subscription of the same provider, and
// getting rid of one. Only shown where it can work: the other providers are read from a login their own tool
// stored, and that store holds exactly one, so offering a second would be a control that cannot do anything.
import { invoke } from "@tauri-apps/api/core";
import { t } from "../../shared/i18n";
import type { AppSettings } from "../../shared/settings";
import { Button, TextField } from "../controls";
import { Group, Row } from "../Group";
import { AddAccountRows } from "./SignIn";
import { renameAccount } from "./store";

export function AccountsGroup({ id, provider, primary, settings, multipleAccounts, onNavigate }: {
  id: string;
  provider: string;
  primary: boolean;
  settings: AppSettings;
  /** `ProviderInfo.multipleAccounts` */
  multipleAccounts: boolean;
  onNavigate: (pane: string) => void;
}) {
  if (!multipleAccounts) return null;
  const extra = settings.extraAccounts.find((a) => a.id === id);

  return (
    <Group title={t("Accounts")}>
      <AddAccountRows id={id} provider={provider} primary={primary} />
      {!primary && extra && (
        <>
          <Row title={t("Name")}>
            <TextField
              label={t("Name")}
              value={extra.name}
              onCommit={(name) => { if (name !== extra.name) void renameAccount(settings, id, name); }}
            />
          </Row>
          <Row title={t("Remove account")} subtitle={t("Forgets its login and takes it off the rail.")}>
            <Button
              onClick={() => {
                // Forgets the stored login as well as the account, then back to a pane that exists.
                void invoke("signin_remove_account", { account: id }).then(() => onNavigate("appearance"));
              }}
            >
              {t("Remove")}
            </Button>
          </Row>
        </>
      )}
    </Group>
  );
}
