// The recap window: the deck one card at a time on the left, and on the right the period, the
// price, the privacy switch and the ways out (upstream RecapWindowView and RecapWindowModel).
//
// Reading is the person's decision, never this window's: with Token spend reading off nothing is
// read and nothing is switched on. The report comes from `recap_report`; the ledgers behind it
// are kept in Rust for half an hour, and a report already fetched is kept here for as long.
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { setLanguage, t } from "../shared/i18n";
import { updateSettings, useSettings, type AppSettings } from "../shared/settings";
import { CardView } from "./cards/CardView";
import { makeDeck, periodTitle, type Deck } from "./deck";
import { copyCard, saveAll, saveCard, type Outcome } from "./export";
import { CARD_HEIGHT, CARD_WIDTH, Geo } from "./kit";
import { priceEntry, priceText } from "./price";
import type { CardId, RecapPayload } from "./types";

const FRESH_FOR = 30 * 60 * 1000;

type Phase = "needsReading" | "loading" | "ready" | "failed";

const query = new URLSearchParams(window.location.search);
// Looking at the cards in a plain browser (`vite`, no app behind it): `?fixture=month` reads
// `ui/public/_fixtures/month.json`, which
//   PULSE_RECAP_SAMPLES=ui/public/_fixtures cargo test -p pulse-core dump_samples -- --ignored
// writes (month, year, month-running, year-running, month-unpriced, month-six-weeks, empty).
// Also `&lang=zh-Hans`, `&price=0`, and `&bare=poster&zoom=0.43&y=0` for one card on its own.
// Compiled out of a production build.
const fixture = import.meta.env.DEV ? query.get("fixture") : null;

/** Settings from the app; in a browser preview (`?fixture=month`), a local stand-in. */
function useRecapSettings(): [AppSettings | null, (patch: Partial<AppSettings>) => void] {
  const live = useSettings();
  const [local, setLocal] = useState<Partial<AppSettings>>({
    readsTokenSpend: true,
    recapMonthlyPrice: Number(query.get("price") ?? 200) || null,
    recapHidesProjects: false,
    language: (query.get("lang") as AppSettings["language"] | null) ?? "en",
  });
  if (fixture) return [local as AppSettings, (patch) => setLocal((old) => ({ ...old, ...patch }))];
  return [live, (patch) => void updateSettings(patch)];
}

async function fetchReport(period: string | null): Promise<RecapPayload | null> {
  if (fixture) {
    const report = await fetch(`/_fixtures/${fixture}.json`).then((r) => r.json());
    const year = report.period.slice(0, 4);
    return { report, offer: { months: [report.period.includes("-") ? report.period : `${year}-09`], years: [year], defaultMonth: `${year}-09`, defaultYear: year } };
  }
  return invoke<RecapPayload | null>("recap_report", { period });
}

/** A browser preview only (`?fixture=month&bare=poster`): one card at full size, nothing around it. */
function Bare({ card }: { card: CardId }) {
  const [settings] = useRecapSettings();
  const [report, setReport] = useState<RecapPayload | null>(null);
  useEffect(() => {
    document.documentElement.style.overflow = "auto";
    document.body.style.overflow = "auto";
    document.getElementById("root")!.style.height = "auto";
    void fetchReport(null).then(setReport);
  }, []);
  if (settings) setLanguage(settings.language);
  if (!report || !settings) return null;
  const deck = makeDeck(report.report, settings.recapMonthlyPrice, settings.recapHidesProjects);
  return (
    <div style={{ zoom: Number(query.get("zoom") ?? 1), marginTop: -Number(query.get("y") ?? 0) }}>
      <CardView deck={deck} card={deck.cards.includes(card) ? card : "poster"} />
    </div>
  );
}

export function App() {
  const bare = import.meta.env.DEV ? query.get("bare") : null;
  return bare ? <Bare card={bare as CardId} /> : <Window />;
}

function Window() {
  const [settings, update] = useRecapSettings();
  if (settings) setLanguage(settings.language);

  const [asked, setAsked] = useState<string | null>(query.get("period"));
  const [phase, setPhase] = useState<Phase>("loading");
  const [payload, setPayload] = useState<RecapPayload | null>(null);
  const [building, setBuilding] = useState(false);
  const [card, setCard] = useState<CardId>("poster");
  const [note, setNote] = useState<string | null>(null);
  const [exporting, setExporting] = useState(false);
  const [priceField, setPriceField] = useState("");
  const cache = useRef(new Map<string, { at: number; day: string; payload: RecapPayload }>());
  const generation = useRef(0);
  const noteTimer = useRef<number>();

  const reads = settings?.readsTokenSpend;

  const load = useCallback(
    async (key: string | null) => {
      const today = new Date().toDateString();
      const kept = key ? cache.current.get(key) : undefined;
      if (kept && Date.now() - kept.at < FRESH_FOR && kept.day === today) {
        setPayload(kept.payload);
        setBuilding(false);
        setPhase("ready");
        return;
      }
      const mine = ++generation.current;
      setBuilding(true);
      try {
        const result = await fetchReport(key);
        if (mine !== generation.current) return;
        if (!result) {
          setPayload(null);
          setPhase("needsReading");
        } else {
          cache.current.set(result.report.period, { at: Date.now(), day: today, payload: result });
          setPayload(result);
          setPhase("ready");
        }
      } catch {
        if (mine !== generation.current) return;
        setPhase("failed");
      } finally {
        if (mine === generation.current) setBuilding(false);
      }
    },
    [],
  );

  // Reading switched on or off, and the first read.
  useEffect(() => {
    if (reads === undefined) return;
    if (!reads) {
      generation.current++;
      cache.current.clear();
      setPayload(null);
      setBuilding(false);
      setPhase("needsReading");
      return;
    }
    setPhase((p) => (p === "ready" ? p : "loading"));
    void load(asked);
    // `asked` is read once here: choosing a period loads it itself.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [reads, load]);

  // The window shown again, perhaps on a period the Token spend pane or a notification named.
  useEffect(() => {
    if (fixture) return;
    const un = listen<string | null>("recap-open", (event) => {
      cache.current.clear();
      const next = event.payload ?? null;
      setAsked(next);
      setCard("poster");
      void load(next);
    });
    return () => void un.then((f) => f());
  }, [load]);

  // The price field follows the stored price.
  useEffect(() => {
    setPriceField(priceText(settings?.recapMonthlyPrice));
  }, [settings?.recapMonthlyPrice]);

  const report = payload?.report ?? null;
  const price = settings?.recapMonthlyPrice ?? null;
  const hides = settings?.recapHidesProjects ?? false;
  const deck = useMemo(() => (report ? makeDeck(report, price, hides) : null), [report, price, hides, settings?.language]);

  const hasDeck = !!deck && deck.cards.length > 0;
  const current: CardId = deck && deck.cards.includes(card) ? card : (deck?.cards[0] ?? "poster");
  const index = deck ? Math.max(deck.cards.indexOf(current), 0) : 0;

  useEffect(() => {
    if (!report) return;
    const title = `${report.period.includes("-") ? t("Monthly Recap") : t("Yearly Recap")} · ${periodTitle(report.period)}`;
    document.title = title;
    try {
      void getCurrentWindow().setTitle(title).catch(() => {});
    } catch {
      // Not inside the app (a browser preview): the document title is all there is.
    }
  }, [report?.period, settings?.language]);

  const step = useCallback(
    (direction: number) => {
      if (!deck) return;
      const target = deck.cards.indexOf(current) + direction;
      if (target >= 0 && target < deck.cards.length) setCard(deck.cards[target]);
    },
    [deck, current],
  );

  // Keys are the window's own: a bare arrow turns the page unless something that uses the arrows
  // itself has the focus (a field, a menu, the Month / Year segments).
  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (event.key !== "ArrowLeft" && event.key !== "ArrowRight") return;
      if (event.altKey || event.ctrlKey || event.metaKey || event.shiftKey) return;
      const target = event.target;
      if (target instanceof HTMLElement && (["INPUT", "SELECT", "TEXTAREA"].includes(target.tagName) || target.isContentEditable || target.closest(".segmented"))) return;
      event.preventDefault();
      step(event.key === "ArrowLeft" ? -1 : 1);
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [step]);

  // MARK: Period

  const choose = (key: string) => {
    if (!report || key === report.period) return;
    setAsked(key);
    setCard("poster");
    void load(key);
  };

  const isYear = deck?.isYear ?? false;
  const offer = payload?.offer;
  const selectKind = (year: boolean) => {
    if (!report || !offer || year === isYear) return;
    if (year) choose(report.period.slice(0, 4));
    else choose(offer.months.find((m) => m.startsWith(report.period)) ?? offer.defaultMonth);
  };
  const offered = useMemo(() => {
    if (!report || !offer) return [];
    const list = isYear ? offer.years : offer.months;
    return list.includes(report.period) ? list : [report.period, ...list];
  }, [report, offer, isYear]);

  // MARK: Price and privacy

  /** Takes what is in the field: nothing clears the price, a positive amount sets it, and
   *  anything else goes back to what was kept. Returns the price now in force. */
  const commitPrice = (): number | null => {
    const entry = priceEntry(priceField);
    if (entry.kind === "none") {
      if (price !== null) update({ recapMonthlyPrice: null });
      setPriceField("");
      return null;
    }
    if (entry.kind === "amount") {
      if (price !== entry.value) update({ recapMonthlyPrice: entry.value });
      setPriceField(priceText(entry.value));
      return entry.value;
    }
    setPriceField(priceText(price));
    return price;
  };

  // MARK: Export

  const show = (outcome: Outcome) => {
    const text = outcome === "saved" ? t("Saved") : outcome === "copied" ? t("Copied") : t("Couldn't make the image.");
    window.clearTimeout(noteTimer.current);
    setNote(text);
    noteTimer.current = window.setTimeout(() => setNote(null), 3000);
  };

  /** The deck as it stands after the price field has been taken: typing a price and pressing a
   *  button straight away must export the payback card. */
  const exportable = (): Deck | null => {
    const effective = commitPrice();
    return report ? makeDeck(report, effective, hides) : null;
  };

  const run = async (work: (deck: Deck) => Promise<Outcome>) => {
    const target = exportable();
    if (!target || target.cards.length === 0) return;
    setExporting(true);
    try {
      show(await work(target));
    } finally {
      setExporting(false);
    }
  };

  // MARK: Stage

  const openTokenSpend = () => void invoke("open_settings");

  const stage = () => {
    if (phase === "needsReading") {
      return (
        <Message
          symbol="chart"
          title={t("The recap needs Token spend")}
          detail={t("It is built from the usage records on this Mac, which Pulse reads only while Token spend reading is on.")}
          action={{ title: t("Open Token spend"), run: openTokenSpend }}
        />
      );
    }
    if (phase === "failed") {
      return <Message symbol="warning" title={t("The records could not be read.")} action={{ title: t("Retry"), run: () => void load(asked) }} />;
    }
    if (phase === "loading" && !report) {
      return (
        <div className="message">
          <div className="spinner" />
          <div className="detail" style={{ fontSize: 13, color: "var(--r-text)" }}>
            {t("Reading…")}
          </div>
          <div className="detail">{t("The first read of a long history can take a minute or two.")}</div>
        </div>
      );
    }
    if (deck && hasDeck) return <Viewer deck={deck} current={current} index={index} onStep={step} onPick={setCard} dim={building} />;
    if (building || !report) return <div className="spinner" />;
    return <Message symbol="tray" title={t("No records for %@", periodTitle(report.period))} detail={t("Nothing in this Mac's usage records falls in this period.")} />;
  };

  return (
    <div className="recap-window">
      <div className="stage">{stage()}</div>
      <div className="sidebar">
        <div className="section">
          <div className="heading">{t("Period")}</div>
          <div className="segmented" role="radiogroup" aria-label={t("Period")}>
            <button className={isYear ? "" : "on"} role="radio" aria-checked={!isYear} onClick={() => selectKind(false)}>
              {t("Month")}
            </button>
            <button className={isYear ? "on" : ""} role="radio" aria-checked={isYear} onClick={() => selectKind(true)}>
              {t("Year")}
            </button>
          </div>
          <select className="select" aria-label={t("Period")} value={report?.period ?? ""} onChange={(e) => choose(e.target.value)} disabled={!report}>
            {offered.map((key) => (
              <option key={key} value={key}>
                {periodTitle(key)}
              </option>
            ))}
          </select>
        </div>

        <div className="section">
          <div className="heading">{t("Monthly price")}</div>
          <div className="price">
            <span className="sign">$</span>
            <input
              aria-label={t("Monthly price")}
              value={priceField}
              spellCheck={false}
              onChange={(e) => setPriceField(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === "Enter") commitPrice();
              }}
              onBlur={() => commitPrice()}
            />
          </div>
          <div className="hint">{t("Only used for the payback card. Leave it empty to leave that card out.")}</div>
        </div>

        <div className="section">
          <div className="heading">{t("Privacy")}</div>
          <div className="toggle">
            <span>{t("Hide project names")}</span>
            <button className={"switch" + (hides ? " on" : "")} role="switch" aria-checked={hides} onClick={() => update({ recapHidesProjects: !hides })} />
          </div>
        </div>

        <div className="grow" />

        <div className="exports">
          <div className="note">{note}</div>
          <div className="buttons">
            <button className="button" disabled={!hasDeck || exporting} onClick={() => void run((d) => saveCard(d, current))}>
              {t("Save image")}
            </button>
            <button className="button" disabled={!hasDeck || exporting} onClick={() => void run((d) => saveAll(d))}>
              {t("Save all")}
            </button>
            <button className="button" disabled={!hasDeck || exporting} onClick={() => void run((d) => copyCard(d, current))}>
              {t("Copy image")}
            </button>
          </div>
        </div>
      </div>
    </div>
  );
}

function Message({ symbol, title, detail, action }: { symbol: "chart" | "warning" | "tray"; title: string; detail?: string; action?: { title: string; run: () => void } }) {
  const paths = {
    chart: <path d="M4 20V10M10 20V4M16 20v-7M22 20H2" />,
    warning: <path d="M12 3 2 21h20L12 3ZM12 10v5M12 18v.5" />,
    tray: <path d="M3 13h5l1.5 3h5L16 13h5M3 13l3-8h12l3 8v6H3v-6Z" />,
  };
  return (
    <div className="message">
      <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.2" strokeLinecap="round" strokeLinejoin="round">
        {paths[symbol]}
      </svg>
      <div className="title">{title}</div>
      {detail ? <div className="detail">{detail}</div> : null}
      {action ? (
        <button className="button" style={{ marginTop: 4 }} onClick={action.run}>
          {action.title}
        </button>
      ) : null}
    </div>
  );
}

/** One card, scaled to fit, with the way to the one before and after. */
function Viewer({ deck, current, index, onStep, onPick, dim }: { deck: Deck; current: CardId; index: number; onStep: (d: number) => void; onPick: (c: CardId) => void; dim: boolean }) {
  const page = deck.page(current);
  const chevron = (d: string) => (
    <svg width="14" height="14" viewBox="0 0 14 14" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
      <path d={d} />
    </svg>
  );
  return (
    <div className="viewer" style={{ opacity: dim ? 0.6 : 1 }}>
      <div className="pager">
        <button className="pager-button" disabled={index <= 0} aria-label={t("Previous page")} onClick={() => onStep(-1)}>
          {chevron("M9 2 4 7l5 5")}
        </button>
        <Geo style={{ flex: "1 1 0" }}>
          {(w, h) => {
            const scale = Math.max(Math.min(w / CARD_WIDTH, h / CARD_HEIGHT), 0.05);
            return (
              <div className="card-clip" style={{ width: CARD_WIDTH * scale, height: CARD_HEIGHT * scale }}>
                <div style={{ width: CARD_WIDTH, height: CARD_HEIGHT, transform: `scale(${scale})`, transformOrigin: "0 0" }}>
                  <CardView deck={deck} card={current} />
                </div>
              </div>
            );
          }}
        </Geo>
        <button className="pager-button" disabled={index >= deck.cards.length - 1} aria-label={t("Next page")} onClick={() => onStep(1)}>
          {chevron("M5 2l5 5-5 5")}
        </button>
      </div>
      <div className="dots">
        <div className="dots-row">
          {deck.cards.map((id) => (
            <button key={id} className={"dot" + (id === current ? " on" : "")} aria-label={id} onClick={() => onPick(id)} />
          ))}
        </div>
        {/* The count printed on the cards themselves: the poster is not one of the numbered stories, so it is named instead. */}
        <span className="counter">{page ? `${page.number} / ${page.count}` : t("Overview")}</span>
      </div>
    </div>
  );
}
