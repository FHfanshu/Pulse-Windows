// Ported from upstream Settings/PromptCacheSessionsView.swift (`PromptCacheSessionsGroup`): every Claude Code or
// Codex conversation whose prompt cache is still alive (for Codex, still inside OpenAI's guaranteed thirty
// minutes), soonest to lapse first, each with the time it has left.
//
// The detailed card names only the soonest; this is where the rest are. Read again every half minute while the
// pane is open, and counted down in between, so a conversation that lapses drops off without waiting for a read.
import { useEffect, useState } from "react";
import { t } from "../../shared/i18n";
import {
  duration, promptCache, sessionLabel, sessionNamesItself, timeText,
  type PromptCacheLapse, type PromptCacheReading, type PromptCacheSession,
} from "../../shared/spend";
import { Group, Row } from "../Group";

/** Past this after a lapse, the latest one is not worth a line. */
const STALE_AFTER_MS = 24 * 3600 * 1000;
/** Red in its last five minutes: a message now still reads the cache; one a little later writes it again. */
const URGENT_SECONDS = 300;

export function PromptCacheSessionsGroup({ provider }: { provider: string }) {
  const [reading, setReading] = useState<{ provider: string; reading: PromptCacheReading | null } | null>(null);
  const [now, setNow] = useState(() => Date.now());

  // Keyed by provider. Moving from Claude Code's pane to Codex's keeps this component where it is, so a bare
  // effect neither restarted nor let go of the reading: Codex's pane listed Claude Code's conversations
  // under Codex's footnote.
  useEffect(() => {
    let live = true;
    const read = () =>
      promptCache(provider)
        .then((r) => live && setReading({ provider, reading: r }))
        .catch(() => {});
    void read();
    const every30s = setInterval(() => void read(), 30_000);
    return () => {
      live = false;
      clearInterval(every30s);
    };
  }, [provider]);

  // Counted down between reads.
  useEffect(() => {
    const timer = setInterval(() => setNow(Date.now()), 30_000);
    return () => clearInterval(timer);
  }, []);

  /** Codex's times are a floor OpenAI guarantees, and are worded as one. */
  const isFloor = provider === "codex";
  const current = reading?.provider === provider ? reading.reading : null;
  const answered = reading?.provider === provider;
  const live = (current?.live ?? [])
    .filter((s) => Date.parse(s.lapse.expiresAt) > now)
    .sort((a, b) => Date.parse(a.lapse.expiresAt) - Date.parse(b.lapse.expiresAt));

  const lapsedLine = (): string | null => {
    const lapsed: PromptCacheLapse | undefined =
      current?.lastLapsed ??
      [...(current?.live ?? [])].map((s) => s.lapse).sort((a, b) => Date.parse(b.expiresAt) - Date.parse(a.expiresAt))[0];
    if (!lapsed || now - Date.parse(lapsed.expiresAt) >= STALE_AFTER_MS) return null;
    return isFloor
      ? t("The latest one's guaranteed time ended at %@.", timeText(lapsed.expiresAt))
      : t("The latest one's cache lapsed at %@.", timeText(lapsed.expiresAt));
  };

  /** "Pulse · 1 hr cache · Last used 14:02". The project only when the title is something else. */
  const subtitle = (session: PromptCacheSession): string => {
    const parts: string[] = [];
    if (sessionNamesItself(session) && session.project) parts.push(session.project);
    parts.push(isFloor ? t("Kept at least %@", duration(session.lapse.lifetime)) : t("%@ cache", duration(session.lapse.lifetime)));
    parts.push(t("Last used %@", timeText(session.lapse.lastRequest)));
    return parts.join(" · ");
  };

  return (
    <div className="pane-sub">
      <Group title={t("Prompt cache")}>
        {!answered ? (
          <Row title={t("Reading local records…")}>
            <span className="spinner" role="progressbar" />
          </Row>
        ) : live.length === 0 ? (
          <Row
            title={isFloor ? t("No conversation is within its guaranteed cache time") : t("No conversation is holding a cache")}
            subtitle={lapsedLine()}
          />
        ) : (
          live.map((session) => {
            const left = (Date.parse(session.lapse.expiresAt) - now) / 1000;
            const text = duration(left);
            return (
              <Row key={session.id} title={sessionLabel(session, session.project)} subtitle={subtitle(session)}>
                <span className="cache-left num" data-urgent={left <= URGENT_SECONDS}>
                  {isFloor ? t("At least %@ left", text) : t("%@ left", text)}
                </span>
              </Row>
            );
          })
        )}
      </Group>
      <div className="footnote">
        {isFloor
          ? t("Read from Codex's own records: the model each request used, and when it was sent. On GPT-5.6 and later, OpenAI keeps a cache available for at least 30 minutes after it was last used, and may keep it longer — so this is the time it is guaranteed, not when it ends. Earlier models have no stated lifetime and are not listed. Under heavy traffic a request can still miss within this time.")
          : t("Read from Claude Code's own records: each reply says whether it cached for an hour or five minutes, and a cache lasts that long from the last request that used it. That is the time if nothing before it has changed — switching model, changing tools or compacting starts a new cache. Subagents keep caches of their own and are not listed.")}
      </div>
    </div>
  );
}
