import type { ReactNode } from "react";

/** A titled, rounded card holding rows (upstream `SettingsGroup`). */
export function Group({ title, children }: { title?: string; children: ReactNode }) {
  return (
    <section>
      {title && <h3 className="group-title">{title}</h3>}
      <div className="group-card">{children}</div>
    </section>
  );
}

interface RowProps {
  title: string;
  subtitle?: string | null;
  /** Renders the subtitle in the error colour. */
  invalid?: boolean;
  icon?: ReactNode;
  disabled?: boolean;
  children?: ReactNode;
}

/** Title and one-line grey subtitle on the left, control on the right (upstream `SettingsRow`). */
export function Row({ title, subtitle, invalid, icon, disabled, children }: RowProps) {
  return (
    <div className="row" data-disabled={disabled ? "true" : undefined}>
      <div className="row-label">
        {icon && <span className="glyph">{icon}</span>}
        <div>
          <div className="row-title">{title}</div>
          {subtitle ? <div className={`row-subtitle${invalid ? " error" : ""}`}>{subtitle}</div> : null}
        </div>
      </div>
      {children !== undefined && <div className="row-control">{children}</div>}
    </div>
  );
}
