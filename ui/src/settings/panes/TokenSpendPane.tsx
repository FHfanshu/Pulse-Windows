import { t } from "../../shared/i18n";
import { Group, Row } from "../Group";

// TODO: ported later (local usage reading, per-agent figures, activity charts).
export function TokenSpendPane() {
  return (
    <div className="pane-stack">
      <Group>
        <Row title={t("Read local usage records")} subtitle={t("Coming soon.")} disabled />
      </Group>
    </div>
  );
}
