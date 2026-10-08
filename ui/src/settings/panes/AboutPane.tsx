import { t } from "../../shared/i18n";
import { Group, Row } from "../Group";

// TODO: ported later (version and update state, source address, credits).
export function AboutPane() {
  return (
    <div className="pane-stack">
      <Group>
        <Row title={t("Version")}><span className="value-text">0.1.0</span></Row>
      </Group>
    </div>
  );
}
