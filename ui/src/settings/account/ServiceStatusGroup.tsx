// Ported from upstream Settings/ServiceStatusGroup.swift: how the provider says its service is doing,
// drawn the way its status page draws it: per component, the state now, one bar a day for about ninety
// days, and the page's uptime figure between "90 days ago" and "Today".
//
// **The provider's word, and said to be.** Pulse does not test the service; the footnote names who reports
// it, the uptime is the page's own figure, and each page keeps its own colours. Read when the pane opens and
// every five minutes while it stays open, never once it is closed. A page that can't be read says so,
// rather than leaving rows to read as healthy.
import { useEffect, useState } from "react";
import { t } from "../../shared/i18n";
import { intlLocale } from "../../shared/spend";
import { Button } from "../controls";
import { Group, Row } from "../Group";
import { useWidth } from "../spend/SpendCharts";
import { openStatusPage, serviceStatus, type StatusComponent, type StatusDay, type StatusPageId, type StatusState, type StatusView } from "./api";
import type { AccountComponentProps } from "./AccountHistoryGroup";
import "./account.css";

/** Providers whose own status page Pulse reads (`StatusPage::for_provider`). */
const statusProviders = new Set(["codex", "claudeCode", "deepSeek"]);
/** `status::CHECK_INTERVAL`: status pages are written by people, minutes into an incident. */
const CHECK_MS = 5 * 60 * 1000;

export function ServiceStatusGroup({ id, provider, settings }: AccountComponentProps) {
  // The service, not the account, so every account of it shows this; a switched-off one fetches nothing.
  const enabled = settings.enabledAccounts.includes(id);
  const active = enabled && statusProviders.has(provider);
  const [view, setView] = useState<StatusView | null>(null);
  const [reading, setReading] = useState(true);

  // Read on opening and again every five minutes while open; leaving the pane cancels it. A later read that
  // fails replaces the rows with "couldn't read" rather than leaving old ones to pass for current.
  useEffect(() => {
    if (!active) return;
    let cancelled = false;
    let timer: ReturnType<typeof setTimeout> | undefined;
    setReading(true);
    setView(null);
    const read = async (first: boolean) => {
      let next: StatusView | null = null;
      try {
        // The first read may be served from a read a moment ago; the later ones are the clock's.
        next = await serviceStatus(provider, !first);
      } catch {
        next = null;
      }
      if (cancelled) return;
      setView(next);
      setReading(false);
      timer = setTimeout(() => void read(false), CHECK_MS);
    };
    void read(true);
    return () => {
      cancelled = true;
      if (timer) clearTimeout(timer);
    };
  }, [active, provider]);

  if (!active) return null;
  const status = view?.status ?? null;
  const page = view?.page ?? null;
  return (
    <section>
      <Group title={t("Service status")}>
        {status && page ? (
          status.components.map((component) => <ComponentRow key={component.id} component={component} page={page} />)
        ) : reading ? (
          <Row title={t("Checking…")}>
            <span className="spinner" role="progressbar" aria-label={t("Checking…")} />
          </Row>
        ) : (
          <Row title={t("Couldn't read the status page.")} />
        )}
        {view && (
          <Row title={t("Status page")} subtitle={view.host}>
            <Button onClick={() => void openStatusPage(provider)}>{t("Open")}</Button>
          </Row>
        )}
      </Group>
      {view && (
        <p className="status-note">
          {t("As %@ reports it on its status page, read when this page opens and every five minutes while it stays open.", view.company)}
        </p>
      )}
    </section>
  );
}

/** One component: its name and state, its days, and the uptime line. */
function ComponentRow({ component, page }: { component: StatusComponent; page: StatusPageId }) {
  return (
    <div className="status-component">
      <div className="status-line">
        <span className="status-name">{component.name}</span>
        <span className="status-state" style={{ color: stateColour(page, component.state) }}>{stateTitle(component.state)}</span>
      </div>
      {component.days.length > 0 && (
        <>
          <DayBars days={component.days} page={page} />
          <div className="status-ends">
            <span>{t("90 days ago")}</span>
            <span className="rule" />
            {component.uptime !== null && (
              <>
                <span>{t("%@ uptime", uptimeText(component.uptime))}</span>
                <span className="rule" />
              </>
            )}
            <span>{t("Today")}</span>
          </div>
        </>
      )}
    </div>
  );
}

/** The page's figure as it gives it: up to two decimals, none for 100. */
function uptimeText(value: number): string {
  return new Intl.NumberFormat(intlLocale(), { style: "percent", minimumFractionDigits: 0, maximumFractionDigits: 2 }).format(value / 100);
}

/** What the pane and a notification call it (upstream `ServiceStatus.State.title`). */
export function stateTitle(state: StatusState): string {
  switch (state) {
    case "operational": return t("Operational");
    case "degraded": return t("Degraded performance");
    case "partialOutage": return t("Partial outage");
    case "fullOutage": return t("Full outage");
    case "maintenance": return t("Under maintenance");
    case "unrecognised": return t("Unrecognised status");
  }
}

// MARK: - Bars

/**
 * A bar a day, oldest on the left, at the page's own proportions: bars three units wide with two between,
 * stretched across the row. Each bar's tooltip is its date and the day's worst state.
 */
function DayBars({ days, page }: { days: StatusDay[]; page: StatusPageId }) {
  const [ref, width] = useWidth<HTMLDivElement>();
  const height = 24;
  const unit = width / Math.max(days.length * 5 - 2, 1);
  const radius = barCornerRadius(page);
  return (
    <div ref={ref} aria-hidden>
      {width > 0 && (
        <svg className="status-bars" width={width} height={height} viewBox={`0 0 ${width} ${height}`}>
          {days.map((day, index) => (
            <rect key={day.date} x={index * 5 * unit} y={0} width={unit * 3} height={height} rx={radius} fill={dayColour(page, day)}>
              <title>{tooltip(day)}</title>
            </rect>
          ))}
        </svg>
      )}
    </div>
  );
}

function tooltip(day: StatusDay): string {
  const state = day.state ? stateTitle(day.state) : t("No data");
  const [y, m, d] = day.date.split("-").map(Number);
  if (!y || !m || !d) return state;
  const date = new Intl.DateTimeFormat(intlLocale(), { year: "numeric", month: "short", day: "numeric" }).format(new Date(y, m - 1, d));
  return `${date} · ${state}`;
}

// MARK: - Palette

/**
 * Each page's own colours, taken off the page (2026-10-04): incident.io's for OpenAI, Claude's Statuspage
 * theme for Claude, Flashcat's for DeepSeek.
 */
const palettes: Record<StatusPageId, Record<Exclude<StatusState, "unrecognised">, number>> = {
  openAi: { operational: 0x24c19a, degraded: 0xfbbf24, partialOutage: 0xf5785c, fullOutage: 0xf87171, maintenance: 0x60a5fa },
  claude: { operational: 0x76ad2a, degraded: 0xfaa72a, partialOutage: 0xe86235, fullOutage: 0xe04343, maintenance: 0x2c84db },
  deepSeek: { operational: 0x22c55e, degraded: 0xeab308, partialOutage: 0xf97316, fullOutage: 0xef4444, maintenance: 0x3b82f6 },
};

const hex = (rgb: number) => `#${rgb.toString(16).padStart(6, "0")}`;

/** incident.io and Flashcat round their bars; Statuspage's are square. */
const barCornerRadius = (page: StatusPageId) => (page === "claude" ? 0 : 1.5);

function stateColour(page: StatusPageId, state: StatusState): string {
  return state === "unrecognised" ? "var(--s-text-2)" : hex(palettes[page][state]);
}

/** The page's own colour when it gave one; otherwise the state's. A day with no record is drawn faint in both light and dark. */
function dayColour(page: StatusPageId, day: StatusDay): string {
  if (day.rgb !== null) return hex(day.rgb);
  if (day.state === null) return "color-mix(in srgb, var(--s-text-2) 20%, transparent)";
  return stateColour(page, day.state);
}
