// Ported from upstream Settings/DeepSeekConsoleGroup.swift and OpenCodeConsoleGroup.swift.
//
// A web console's signed-in session, kept beside the API key and never instead of it (each has its own slot in
// the store, so reading one can never clear the other): DeepSeek's reads the account's usage day by day and the
// balance; OpenCode's reads the request log and the plan's limits.
//
// Windows: there is no browser to read. "Read" opens a sign-in window of Pulse's own (src-tauri/src/console_ipc.rs);
// a session it already holds is taken without asking anybody to sign in twice.
//
// Shown for the first account of the provider while it is on the panel (upstream: `provider == .openCodeGo /
// .deepSeek, account.isPrimary, settings.isEnabled(account)`).
import { useEffect, useState } from "react";
import { t } from "../../shared/i18n";
import { Button } from "../controls";
import { Group, Row } from "../Group";
import type { AccountComponentProps } from "./AccountHistoryGroup";
import { CONSOLE_SESSION_EVENT, consoleRead, consoleRemove, consoleSession, type ConsoleProvider, type ConsoleRead } from "./consoles";
import "./account.css";
import "./consoles.css";

interface Props extends AccountComponentProps {
  /** Tells the pane the sign-in changed, so it shows (or drops) the account's history. Also fired as a window event. */
  onSessionChange?: () => void;
}

interface Console {
  provider: ConsoleProvider;
  groupTitle: () => string;
  rowTitle: () => string;
  kept: () => string;
  hint: () => string;
  notFound: () => string;
  footnote: () => string;
  /** The message after a read that was accepted. */
  read: (outcome: Extract<ConsoleRead, { kind: "read" }>) => string;
  /** OpenCode's request log is shown on the detailed card: the hint says so while that is still off. */
  cardHint?: (settings: Props["settings"], id: string) => string | null;
}

/** What the session was read from, said as an argument of the sentences upstream words for a browser. */
const source = () => t("Pulse's sign-in window");

const deepSeek: Console = {
  provider: "deepSeek",
  groupTitle: () => t("DeepSeek console"),
  rowTitle: () => t("Account usage"),
  kept: () => t("Kept. The usage history below and the detailed card read the whole account with it."),
  hint: () => t("Opens a window where you sign in to platform.deepseek.com."),
  notFound: () => t("No DeepSeek console sign-in found. Sign in at platform.deepseek.com first."),
  footnote: () =>
    t("The usage DeepSeek's console keeps for the account — every key and every machine, day by day, and what was charged — and the balance, when no key is set or the key is turned away. Pulse keeps only the console's sign-in, encrypted on this Mac."),
  read: () => t("Read from %@.", source()),
};

const openCode: Console = {
  provider: "openCodeGo",
  groupTitle: () => t("OpenCode console"),
  rowTitle: () => t("Account request log"),
  kept: () => t("Kept. The detailed card reads every request on the account with it."),
  hint: () => t("Opens a window where you sign in to opencode.ai."),
  notFound: () => t("No OpenCode console session found. Sign in at opencode.ai first."),
  footnote: () =>
    t("The request log OpenCode's console keeps for 30 days — every machine and every app on the account, and what each request cost — and the plan's limits, when no key is set or the key is turned away. Pulse keeps only the console's sign-in cookies, encrypted on this Mac."),
  read: (outcome) => t("Read from %@ · workspace %@.", source(), outcome.workspace ?? ""),
  // Where the log will be seen, while that is still switched off: the card is the only place it shows, and a
  // read that changed nothing on screen reads as a read that did nothing.
  cardHint: (settings, id) =>
    settings.detailedCards.includes(id) ? null : t("Turn on Detailed card above to see the request log on the card."),
};

function ConsoleGroup({ id, provider, settings, onSessionChange, console }: Props & { console: Console }) {
  const [hasSession, setHasSession] = useState(false);
  const [message, setMessage] = useState<string | null>(null);
  const [reading, setReading] = useState(false);
  const shown = id === provider && provider === console.provider && settings.enabledAccounts.includes(id);

  useEffect(() => {
    if (!shown) return;
    let live = true;
    consoleSession(console.provider).then((kept) => live && setHasSession(kept)).catch(() => {});
    return () => {
      live = false;
    };
  }, [shown, console.provider]);

  if (!shown) return null;

  const changed = () => {
    onSessionChange?.();
    window.dispatchEvent(new CustomEvent(CONSOLE_SESSION_EVENT, { detail: { provider: console.provider } }));
  };

  const read = async () => {
    setReading(true);
    try {
      const outcome = await consoleRead(console.provider);
      switch (outcome.kind) {
        case "read":
          setMessage(console.read(outcome));
          setHasSession(true);
          changed();
          break;
        case "unanswered":
          // Kept, but the console did not answer; the next refresh tries again.
          setMessage(t("Read from %@, but the console didn't answer. It will be tried again on the next refresh.", source()));
          setHasSession(true);
          changed();
          break;
        case "notFound":
          setMessage(console.notFound());
          break;
        case "busy":
          break;
      }
    } catch {
      setMessage(console.notFound());
    } finally {
      setReading(false);
    }
  };

  const remove = async () => {
    try {
      await consoleRemove(console.provider);
    } finally {
      setHasSession(false);
      setMessage(null);
      changed();
    }
  };

  const subtitle = [message ?? (hasSession ? console.kept() : console.hint()), hasSession ? console.cardHint?.(settings, id) : null]
    .filter(Boolean)
    .join(" ");

  return (
    <div className="signals-stack">
      <Group title={console.groupTitle()}>
        <Row title={console.rowTitle()} subtitle={subtitle}>
          {reading && <span className="spinner" role="progressbar" aria-label={t("Reading…")} />}
          {hasSession && <Button onClick={() => void remove()}>{t("Remove")}</Button>}
          <Button disabled={reading} onClick={() => void read()}>{t("Read")}</Button>
        </Row>
      </Group>
      <div className="footnote">
        <div>{console.footnote()}</div>
      </div>
    </div>
  );
}

export function DeepSeekConsoleGroup(props: Props) {
  return <ConsoleGroup {...props} console={deepSeek} />;
}

export function OpenCodeConsoleGroup(props: Props) {
  return <ConsoleGroup {...props} console={openCode} />;
}
