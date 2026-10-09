// Ported from upstream Settings/AccountPanelGroup.swift. The bot-mark rows (Animated mark, personality, colour,
// shape) are not ported: the animated mark is removed from this port by decision (docs/SPEC.md §2).
import { useEffect, useRef, useState } from "react";
import { windowName } from "../../shared/copy";
import { t } from "../../shared/i18n";
import type { ProviderUsage } from "../../shared/model";
import { recordsAccount, type AppSettings } from "../../shared/settings";
import { Segmented, Select } from "../controls";
import { Group, Row } from "../Group";
import { ToggleRow } from "../panes/ToggleRow";
import { metaOf, pc } from "./meta";
import { setDetailedCard, setEnabled, setPinnedWindow, setRingTint, setSplit } from "./store";

/** Upstream `RingTint.suggestions.first`: a visibly chosen colour to land on when Custom is picked. */
const FIRST_SUGGESTION = "#4099FF";

/** An account's Panel card: whether it is on the rail, which limit its ring follows, its detailed card, and its ring colour. */
export function PanelGroup({ id, provider, settings, usage }: {
  id: string;
  provider: string;
  primary: boolean;
  settings: AppSettings;
  usage: ProviderUsage | undefined;
}) {
  const enabled = settings.enabledAccounts.includes(id);
  const meta = metaOf(provider);
  const tint = settings.ringTints[id] ?? null;

  return (
    <Group title={t("Panel")}>
      <ToggleRow
        title="Show in panel"
        checked={enabled}
        // The last one standing can't be switched off: an empty rail has nothing to hover and nothing to drag.
        disabled={enabled && settings.enabledAccounts.length === 1}
        onChange={(on) => void setEnabled(settings, id, on)}
      />
      <RingWindowRow id={id} settings={settings} usage={usage} />
      {/* Only where there is more than one budget to split. Every other provider reports one pool, and a switch
          that promises a second ring it can never draw is worse than no switch. */}
      {meta.splitsByModelGroup && (
        <ToggleRow
          title="A ring for each model group"
          subtitle="Gemini and the third-party models draw on separate allowances. One ring can only follow the busier of the two."
          checked={settings.splitAccounts.includes(id)}
          onChange={(on) => void setSplit(settings, id, on)}
        />
      )}
      {/* TODO(opus): "Reset credits on the card" (Codex's first account, `showsCodexResetCredits`): needs the setting in
          pulse-core and a Codex app-server read of the limit reset credits. */}
      <ToggleRow
        title="Detailed card"
        subtitle={detailedCardSubtitle(recordsAccount(settings, provider) === id && meta.keepsLocalTranscripts ? "transcripts" : null)}
        checked={settings.detailedCards.includes(id)}
        onChange={(on) => void setDetailedCard(settings, id, on)}
      />
      <Row
        title={t("Ring colour")}
        // Which state it is in, said outright: a colour well always shows *a* colour, so on its own it cannot
        // tell "automatic" from "they picked green".
        subtitle={tint == null ? t("Coloured by how much is left.") : t("A colour of your own, whatever the usage.")}
      >
        <Segmented
          label={t("Ring colour")}
          value={tint == null ? "automatic" : "custom"}
          options={[
            { value: "automatic", label: t("Automatic") },
            { value: "custom", label: t("Custom") },
          ]}
          // Switching on lands on something visibly chosen rather than on the colour the automatic mode happened to show.
          onChange={(v) => void setRingTint(settings, id, v === "custom" ? FIRST_SUGGESTION : null)}
        />
      </Row>
      {/* Only when there is a colour to change. Shown otherwise it is a control that contradicts the row above it. */}
      {tint != null && (
        <Row title={t("Colour")} subtitle={tint.toUpperCase()}>
          <ColourWell value={tint} label={t("Colour")} onCommit={(hex) => void setRingTint(settings, id, hex)} />
        </Row>
      )}
    </Group>
  );
}

/**
 * Which of the provider's limits the rail's ring shows. The options are whatever the provider is reporting right
 * now, so the list changes as limits come and go. A pin that stops matching falls back to the automatic choice.
 */
function RingWindowRow({ id, settings, usage }: { id: string; settings: AppSettings; usage: ProviderUsage | undefined }) {
  const windows = usage?.windows ?? [];
  const pinned = settings.pinnedWindows[id];
  const shown = windows.some((w) => w.id === pinned) ? pinned : "";
  return (
    <Row title={t("Ring shows")} subtitle={t("Which limit the rail's ring tracks.")}>
      <Select
        label={t("Ring shows")}
        value={shown}
        disabled={windows.length === 0}
        options={[
          { value: "", label: t("Highest usage") },
          ...windows.map((w) => ({ value: w.id, label: windowName(w) })),
        ]}
        onChange={(v) => void setPinnedWindow(settings, id, v || null)}
      />
    </Row>
  );
}

/** What the detailed card adds, said the way it will happen (upstream `detailedCardSubtitle`). */
export function detailedCardSubtitle(history: "transcripts" | null): string {
  return history === "transcripts"
    ? pc("Adds the plan, when the figures were read and, with Token spend on, this Mac's recent activity.")
    : t("Adds the plan and when the figures were read.");
}

/** The system colour picker, committed shortly after the last change rather than on every drag step. */
function ColourWell({ value, label, onCommit }: { value: string; label: string; onCommit: (hex: string) => void }) {
  const [draft, setDraft] = useState(value);
  const timer = useRef<number | undefined>(undefined);
  useEffect(() => setDraft(value), [value]);
  useEffect(() => () => window.clearTimeout(timer.current), []);
  return (
    <input
      type="color"
      className="colour-well"
      aria-label={label}
      value={/^#[0-9a-f]{6}$/i.test(draft) ? draft.toLowerCase() : "#4099ff"}
      onChange={(e) => {
        const hex = e.target.value.toUpperCase();
        setDraft(hex);
        window.clearTimeout(timer.current);
        timer.current = window.setTimeout(() => onCommit(hex), 150);
      }}
    />
  );
}
