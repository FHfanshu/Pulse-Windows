import { Switch } from "../controls";
import { Row } from "../Group";
import { t } from "../../shared/i18n";

/** A row whose control is one switch. `title` and `subtitle` are upstream keys. */
export function ToggleRow({ title, subtitle, checked, onChange, disabled }: {
  title: string;
  subtitle?: string;
  checked: boolean;
  onChange: (v: boolean) => void;
  disabled?: boolean;
}) {
  return (
    <Row title={t(title)} subtitle={subtitle ? t(subtitle) : undefined} disabled={disabled}>
      <Switch label={t(title)} checked={checked} disabled={disabled} onChange={onChange} />
    </Row>
  );
}
