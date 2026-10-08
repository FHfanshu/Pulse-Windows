import { t } from "../../shared/i18n";
import { Group, Row } from "../Group";

// TODO: ported later (warn threshold, reset, failure, service-down and recap alerts).
export function NotificationsPane() {
  return (
    <div className="pane-stack">
      <Group>
        <Row title={t("Warn at")} subtitle={t("Coming soon.")} disabled />
      </Group>
    </div>
  );
}
