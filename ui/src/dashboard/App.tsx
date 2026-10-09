// Ported from upstream App/MenuDashboard.swift (the tabbed view) and AppDelegate.addDashboard (the items under it).
// A tab per account on the rail and an overview, each drawn from the figures the rail already has. Nothing here
// fetches a limit; the only read this starts is the spend ledger, and only with Token spend switched on.
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { useEffect, useLayoutEffect, useRef, useState, type ReactNode } from "react";
import { resetText, unavailableMessage, windowName } from "../shared/copy";
import { setLanguage, t } from "../shared/i18n";
import { accountId, headlineWindow, isSpent, percentText, remainingFraction, type ProviderUsage, type UsageWindow } from "../shared/model";
import { updateSettings, useSettings, useUsage, type AppSettings } from "../shared/settings";
import { intlLocale, money, tokensText, chartDate, exactTokens } from "../shared/spend";
import { spendProviders } from "../panel/cardLedgers";
import { ProviderIcon, providerNames } from "../panel/Icon";
import { usageColor } from "../panel/tint";
import { readSpend, forgetSpend, useSpend, type SpendFigure } from "./spend";
import { usagePage } from "./usagePages";

const WIDTH = 320;
/// Few enough accounts that each tab can carry its name; past this the marks carry it (upstream `labelled`).
const LABELLED_UP_TO = 5;
const SPAN = 31;

type Reading = { window: UsageWindow | null; money: string | null; isAlert: boolean };

/** Upstream `MenuBarReading.of`: the ring's limit, else the money, else nothing — never a zero for a missing reading. */
function readingOf(u: ProviderUsage, pinned: string | undefined, warningAt: number): Reading {
  if (u.state.kind === "unavailable") return { window: null, money: null, isAlert: false };
  const window = headlineWindow(u, pinned);
  if (window && !window.estimate) {
    return { window, money: null, isAlert: isSpent(window) || window.usedFraction >= warningAt };
  }
  return { window: null, money: u.creditBalance, isAlert: false };
}

/** "Updated 3 min. ago" (upstream `MenuAccountDetail.updated`). */
function updatedText(iso: string): string {
  const seconds = (Date.parse(iso) - Date.now()) / 1000;
  if (-seconds < 60) return t("Updated just now");
  const rtf = new Intl.RelativeTimeFormat(intlLocale(), { numeric: "auto", style: "short" });
  const abs = Math.abs(seconds);
  const ago = abs < 3600 ? rtf.format(Math.round(seconds / 60), "minute")
    : abs < 86_400 ? rtf.format(Math.round(seconds / 3600), "hour")
    : rtf.format(Math.round(seconds / 86_400), "day");
  return t("Updated %@", ago);
}

const hide = () => void invoke("dashboard_hide");

export function App() {
  const settings = useSettings();
  const { usages } = useUsage();
  const [selected, setSelected] = useState<string | null>(null);
  // Bumped on every opening so "Updated 3 min. ago" and "Resets 4:00 PM" are read again.
  const [, setOpenings] = useState(0);
  const flyout = useRef<HTMLDivElement>(null);
  const ready = settings !== null;

  useEffect(() => {
    const un = [listen("dashboard-shown", () => setOpenings((n) => n + 1))];
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") hide();
    };
    window.addEventListener("keydown", onKey);
    return () => {
      un.forEach((p) => p.then((f) => f()));
      window.removeEventListener("keydown", onKey);
    };
  }, []);

  const readsSpend = settings?.readsTokenSpend ?? false;
  useEffect(() => {
    if (!readsSpend) forgetSpend();
  }, [readsSpend]);

  // The menu grows with its tab: tell the window how tall the content is.
  useLayoutEffect(() => {
    const el = flyout.current;
    if (!el) return;
    const report = () => void invoke("dashboard_resize", { width: WIDTH, height: Math.ceil(el.getBoundingClientRect().height) });
    report();
    const observer = new ResizeObserver(report);
    observer.observe(el);
    return () => observer.disconnect();
  }, [ready]);

  if (!settings) return null;
  setLanguage(settings.language);

  const label = (u: ProviderUsage) =>
    settings.extraAccounts.find((a) => a.id === accountId(u.account))?.name || providerNames[u.account.provider] || u.account.provider;
  // A remembered account that has since left the rail falls back to the overview.
  const account = usages.find((u) => accountId(u.account) === selected) ?? null;
  const labelled = usages.length <= LABELLED_UP_TO;

  return (
    <div className="flyout" ref={flyout} style={{ width: WIDTH }}>
      {usages.length > 0 && (
        <>
          <div className="tabs" role="tablist">
            <Tab selected={!account} label={t("Overview")} labelled={labelled} onClick={() => setSelected(null)}>
              <OverviewMark />
            </Tab>
            {usages.map((u) => {
              const id = accountId(u.account);
              return (
                <Tab key={id} selected={account !== null && accountId(account.account) === id} label={label(u)} labelled={labelled} onClick={() => setSelected(id)}>
                  <ProviderIcon provider={u.account.provider} size={15} />
                </Tab>
              );
            })}
          </div>
          <div className="divider inset" />
          <div className="body">
            {account ? (
              <Detail usage={account} label={label(account)} settings={settings} />
            ) : (
              <Overview usages={usages} label={label} settings={settings} onSelect={setSelected} />
            )}
          </div>
          <div className="divider" />
        </>
      )}
      <Items settings={settings} page={account ? usagePage(account.account.provider) : null} provider={account ? providerNames[account.account.provider] ?? account.account.provider : null} />
    </div>
  );
}

// MARK: - Tabs

function OverviewMark() {
  // Lucide "layout-grid": upstream's square.grid.2x2.
  return (
    <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden>
      <rect width="7" height="7" x="3" y="3" rx="1" /><rect width="7" height="7" x="14" y="3" rx="1" />
      <rect width="7" height="7" x="14" y="14" rx="1" /><rect width="7" height="7" x="3" y="14" rx="1" />
    </svg>
  );
}

function Tab({ selected, label, labelled, onClick, children }: { selected: boolean; label: string; labelled: boolean; onClick: () => void; children: ReactNode }) {
  return (
    <button type="button" role="tab" aria-selected={selected} className={`tab${selected ? " selected" : ""}${labelled ? " labelled" : ""}`} title={label} onClick={onClick}>
      <span className="tab-mark">{children}</span>
      {labelled && <span className="tab-label">{label}</span>}
    </button>
  );
}

// MARK: - Overview

function Overview({ usages, label, settings, onSelect }: { usages: ProviderUsage[]; label: (u: ProviderUsage) => string; settings: AppSettings; onSelect: (id: string) => void }) {
  const warningAt = settings.warningThreshold / 100;
  return (
    <div className="overview">
      {usages.map((u) => {
        const id = accountId(u.account);
        const reading = readingOf(u, settings.pinnedWindows[id], warningAt);
        const w = reading.window;
        const text = w ? percentText(w, settings.showsRemaining) : reading.money ?? "–";
        const detail = w
          ? [windowName(w), resetText(w)].filter(Boolean).join(" · ")
          : reading.money === null ? t("No reading") : null;
        return (
          <button type="button" key={id} className="overview-row" onClick={() => onSelect(id)}>
            <span className="row-mark"><ProviderIcon provider={u.account.provider} size={15} /></span>
            <span className="row-text">
              <span className="row-line">
                <span className="row-name ellipsis">{label(u)}</span>
                <span className={`row-figure${reading.isAlert ? " alert" : ""}`}>{text}</span>
              </span>
              {detail && <span className="row-detail ellipsis">{detail}</span>}
            </span>
          </button>
        );
      })}
    </div>
  );
}

// MARK: - One account

/** Its plan and how fresh the reading is, every limit with its bar and reset, what else the provider reports, and — with
 *  Token spend on — what the local transcripts say it has cost. */
function Detail({ usage, label, settings }: { usage: ProviderUsage; label: string; settings: AppSettings }) {
  const remaining = settings.showsRemaining;
  const warningAt = settings.warningThreshold / 100;
  const provider = usage.account.provider;
  // Only the providers whose transcripts are on this PC, only the account the CLI is signed in to, and only with the
  // reader's own switch on.
  const showsSpend = settings.readsTokenSpend && usage.account.slot === "" && spendProviders.has(provider);

  useEffect(() => {
    if (showsSpend) readSpend(provider);
  }, [showsSpend, provider, usage.account.slot]);

  return (
    <div className="detail">
      <div className="detail-header">
        <div className="row-line">
          <span className="detail-title ellipsis">{label}</span>
          {usage.plan && <span className="detail-plan ellipsis">{usage.plan}</span>}
        </div>
        {usage.observedAt && <span className="caption">{updatedText(usage.observedAt)}</span>}
      </div>

      {usage.state.kind === "unavailable" && <p className="unavailable">{unavailableMessage(usage.state.reason)}</p>}

      {usage.windows.map((w) => <Limit key={w.id} window={w} remaining={remaining} warningAt={warningAt} />)}

      {usage.creditBalance && (
        <div className="credit">
          <span className="limit-name">{usage.creditRemaining === null ? t("Usage credits") : t("Balance")}</span>
          <span className="caption strong">{t("%@ remaining", usage.creditBalance)}</span>
        </div>
      )}

      {showsSpend && (
        <>
          <div className="divider flush" />
          <Spend provider={provider} />
        </>
      )}
    </div>
  );
}

function Limit({ window: w, remaining, warningAt }: { window: UsageWindow; remaining: boolean; warningAt: number }) {
  const spent = isSpent(w);
  const percent = percentText(w, remaining);
  const fraction = Math.min(Math.max(remaining && !spent ? remainingFraction(w) : w.usedFraction, 0), 1);
  return (
    <div className="limit">
      <div className="row-line">
        <span className="limit-name ellipsis">{windowName(w)}</span>
        <span className="caption ellipsis">{resetText(w)}</span>
      </div>
      <div className="bar"><div className="bar-fill" style={{ width: `${fraction * 100}%`, background: usageColor(w.usedFraction, spent, warningAt) }} /></div>
      <span className={`caption strong${spent ? " alert" : ""}`}>{remaining ? t("%@ Left", percent) : t("%@ Used", percent)}</span>
    </div>
  );
}

// MARK: - Spend

function Spend({ provider }: { provider: string }) {
  const { spend, reading } = useSpend(provider);
  if (!spend) {
    return <span className="caption">{reading ? t("Reading local records…") : t("No history yet")}</span>;
  }
  const figure = (title: string, f: SpendFigure) => (
    <div className="figure">
      <span className="caption">{title}</span>
      <span className="figure-cost">{f.cost === null ? "—" : money(f.cost, spend.currency)}</span>
      <span className="caption">{tokensText(f.tokens)}</span>
    </div>
  );
  const peak = Math.max(...spend.bars.map((b) => b.tokens), 1);
  return (
    <div className="spend">
      <div className="figures">
        <div className="figure-column">
          {figure(t("Today"), spend.today)}
          {figure(t("Busiest day"), spend.busiest)}
        </div>
        <div className="figure-column right">
          {figure(t("Last 31 days"), spend.recent)}
          {figure(t("All time"), spend.allTime)}
        </div>
      </div>
      {spend.bars.length > 0 && (
        <div className="chart" role="img" aria-label={t("Tokens per day")} style={{ gridTemplateColumns: `repeat(${SPAN}, 1fr)` }}>
          {spend.bars.map((b) => (
            // A day with any work keeps a visible stub, so a quiet day reads as quiet rather than missing.
            <span
              key={b.date}
              className={`bar-day${b.tokens > 0 ? " active" : ""}`}
              style={{ height: b.tokens > 0 ? Math.max((44 * b.tokens) / peak, 4) : 2 }}
              title={`${chartDate(b.date)} · ${exactTokens(b.tokens)}`}
            />
          ))}
        </div>
      )}
      <span className="footnote">{t("Estimated from token counts at API prices — not your subscription bill.")}</span>
    </div>
  );
}

// MARK: - Items

function Item({ icon, children, checked, onClick }: { icon?: ReactNode; children: ReactNode; checked?: boolean; onClick: () => void }) {
  return (
    <button type="button" className="item" onClick={() => { onClick(); hide(); }}>
      <span className="item-mark">{checked ? <CheckMark /> : icon}</span>
      <span className="ellipsis">{children}</span>
    </button>
  );
}

/** The items that go with whichever tab is open (upstream `addDashboard` + `populateMenu`). */
function Items({ settings, page, provider }: { settings: AppSettings; page: string | null; provider: string | null }) {
  return (
    <div className="items">
      {page && provider && <Item icon={<Glyph d={LINK} />} onClick={() => void invoke("open_url", { url: page })}>{t("Open %@ usage page", provider)}</Item>}
      <Item icon={<Glyph d={REFRESH} />} onClick={() => void invoke("refresh", { account: null })}>{t("Refresh")}</Item>
      <div className="divider inset" />
      <Item checked={settings.isPanelVisible} onClick={() => void updateSettings({ isPanelVisible: !settings.isPanelVisible })}>{t("Show floating panel")}</Item>
      <Item onClick={() => void invoke("open_settings", { pane: null })}>{t("Settings…")}</Item>
      <div className="divider inset" />
      <Item onClick={() => void invoke("quit_app")}>{t("Quit Pulse")}</Item>
    </div>
  );
}

// Lucide "external-link", "refresh-cw", "check".
const LINK = "M15 3h6v6 M10 14 21 3 M18 13v6a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V8a2 2 0 0 1 2-2h6";
const REFRESH = "M3 12a9 9 0 0 1 9-9 9.75 9.75 0 0 1 6.74 2.74L21 8 M21 3v5h-5 M21 12a9 9 0 0 1-9 9 9.75 9.75 0 0 1-6.74-2.74L3 16 M8 16H3v5";

function Glyph({ d }: { d: string }) {
  return (
    <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden>
      <path d={d} />
    </svg>
  );
}

const CheckMark = () => <Glyph d="M20 6 9 17l-5-5" />;
