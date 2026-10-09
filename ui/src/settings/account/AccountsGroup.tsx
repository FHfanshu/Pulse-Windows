// Ported from upstream Settings/AccountsGroup.swift: signing in to another subscription of the same provider, and
// getting rid of one. Only shown where it can work: the other providers are read from a login their own tool
// stored, and that store holds exactly one, so offering a second would be a control that cannot do anything.
import { t } from "../../shared/i18n";
import type { AppSettings } from "../../shared/settings";
import { Button, TextField } from "../controls";
import { Group, Row } from "../Group";
import { removeAccount, renameAccount } from "./store";

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
      <Row
        title={primary ? t("Add another account") : t("Sign in again…")}
        // The one thing someone should know before they start: whose name is on the page that opens.
        subtitle={t("Opens the provider's own sign-in page.")}
      >
        {/* TODO(opus): sign-in flow. One sign-in at a time, with its Cancel, device code (Code row with Copy and
            Open page) and error (Sign-in row) belonging to the provider it was started for. It saves the token
            under a new `provider#slot` account id and adds it to `extraAccounts` and `enabledAccounts`. */}
        <Button disabled onClick={() => {}}>{t("Sign in…")}</Button>
      </Row>
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
                // Back to a pane that exists, once the account is gone.
                void removeAccount(settings, id, provider).then(() => onNavigate("appearance"));
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
