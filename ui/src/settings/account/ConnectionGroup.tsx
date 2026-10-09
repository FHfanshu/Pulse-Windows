// Ported from upstream Settings/AccountConnectionGroup.swift: where a provider's figures come from, plus anything
// that route needs setting up.
//
// Not drawn when it would be empty. An added account of a provider with one route has nothing here, and a
// "Connection" heading over an empty box reads as a control that failed to load.
import { invoke } from "@tauri-apps/api/core";
import { useEffect, useState } from "react";
import { t } from "../../shared/i18n";
import type { ProviderUsage } from "../../shared/model";
import type { AppSettings } from "../../shared/settings";
import { Button, Select } from "../controls";
import { Group, Row } from "../Group";
import { BalanceBasisRow, BalanceBudgetRow, ServerAddressRow } from "./ConnectionRows";
import { hasConnectionControls, metaOf, pc, type BrowserId, type CredentialKind, type UsageSourceId } from "./meta";
import { StatusLineRow } from "./StatusLineRow";
import { setSessionBrowser, setSource } from "./store";

const sourceTitle: Record<UsageSourceId, string> = {
  automatic: "Automatic",
  endpoint: "Usage endpoint",
  tooling: "Provider tooling",
};

/** What a route actually is, which differs per provider (upstream `UsageSource.detail(for:)`). */
function sourceDetail(source: UsageSourceId, provider: string): string {
  if (source === "automatic") return t("Use the endpoint when possible, the other route when not.");
  if (source === "endpoint") {
    return provider === "claudeCode"
      ? t("Reads your account's limits with the login Claude Code saved.")
      : t("Reads your account's limits with the login Codex saved.");
  }
  return provider === "claudeCode"
    ? t("Uses whatever Claude Code last reported to its status line.")
    : t("Asks the Codex app server, which signs in on its own.");
}

const browserName: Record<BrowserId, string> = {
  firefox: "Firefox",
  chrome: "Chrome",
  edge: "Edge",
  brave: "Brave",
  vivaldi: "Vivaldi",
};
const chromium: BrowserId[] = ["chrome", "edge", "brave", "vivaldi"];

export function ConnectionGroup({ id, provider, primary, settings, usage }: {
  id: string;
  provider: string;
  primary: boolean;
  settings: AppSettings;
  usage: ProviderUsage | undefined;
}) {
  if (!hasConnectionControls(provider, primary)) return null;
  const meta = metaOf(provider);
  const source = ((settings.sources[id] as UsageSourceId | undefined) ?? "automatic") as UsageSourceId;
  // A stored route the provider doesn't offer resolves to automatic rather than being handed on.
  const shownSource: UsageSourceId = source === "endpoint" || source === "tooling" ? source : "automatic";

  return (
    <div id="connection">
      <Group title={t("Connection")}>
        {/* A provider with a single route gets told, not asked: a picker with one entry cannot do anything. */}
        {meta.sourceChoice ? (
          <Row title={t("Read usage from")} subtitle={sourceDetail(shownSource, provider)}>
            <Select<UsageSourceId>
              label={t("Read usage from")}
              value={shownSource}
              // The desktop-app route is part of "Provider tooling" in this port's Claude Code reader.
              options={(["automatic", "endpoint", "tooling"] as UsageSourceId[]).map((s) => ({ value: s, label: t(sourceTitle[s]) }))}
              onChange={(s) => void setSource(settings, id, s)}
            />
          </Row>
        ) : meta.githubSignIn ? (
          <GitHubRow id={id} />
        ) : null}

        {/* Above the key, because it is asked first: nothing can be sent anywhere until Pulse knows where. */}
        {meta.serverAddress && <ServerAddressRow id={id} settings={settings} />}

        {meta.credential ? (
          <>
            <CredentialRow id={id} provider={provider} kind={meta.credential} />
            {/* Only where a browser session *is* the credential. Every other provider borrows a login its own
                tool stored, and none of them should be going through anybody's cookies to do it. */}
            {meta.credential !== "apiKey" && (
              <BrowserRow id={id} kind={meta.credential} settings={settings} />
            )}
          </>
        ) : (
          /* One route, so it is stated rather than offered. */
          meta.soleRoute && (
            <Row title={t("Read usage from")} subtitle={t(meta.soleRoute.note)}>
              <span className="value-text route-name">{t(meta.soleRoute.name)}</span>
            </Row>
          )
        )}

        {/* Every API account that reports money: the ring has no denominator until one of three is chosen. */}
        {meta.balanceRing && hasBalanceRing(usage) && (
          <>
            <BalanceBasisRow id={id} settings={settings} />
            {(settings.balanceBases[id] ?? "sinceTopUp") === "budget" && <BalanceBudgetRow id={id} settings={settings} />}
          </>
        )}

        {/* The status line has to be registered before it can report anything, so the control for that follows the
            choice that needs it. */}
        {provider === "claudeCode" && shownSource !== "endpoint" && <StatusLineRow />}
      </Group>
    </div>
  );
}

/** An API account whose reading is money, not limits of its own (upstream `AccountEntryFields.hasBalanceRing`). */
function hasBalanceRing(usage: ProviderUsage | undefined): boolean {
  return (usage?.windows ?? []).every((w) => w.estimate === "sinceTopUp" || w.estimate === "yourBudget");
}

/** Copilot's GitHub account: a sign-in, not a pasted token. */
function GitHubRow({ id }: { id: string }) {
  const [saved, setSaved] = useState(false);
  useEffect(() => {
    invoke<boolean>("has_secret", { id }).then(setSaved).catch(() => {});
  }, [id]);
  const signOut = async () => {
    await invoke("set_secret", { id, value: null });
    setSaved(false);
  };
  return (
    <Row
      title={t("GitHub account")}
      subtitle={saved
        ? pc("Signed in. Pulse holds a read-only token for this Mac.")
        : t("Opens GitHub's own page. Pulse asks to read your profile, nothing else.")}
    >
      {saved ? (
        <Button onClick={() => void signOut()}>{t("Sign out")}</Button>
      ) : (
        // TODO(opus): sign-in flow (GitHub device code: show the code, Copy and Open page rows while it waits).
        <Button disabled onClick={() => {}}>{t("Sign in…")}</Button>
      )}
    </Row>
  );
}

function credentialTitle(kind: CredentialKind): string {
  switch (kind) {
    case "sessionCookie": return t("Session cookie");
    case "browserSession": return t("Browser session");
    default: return t("API key");
  }
}

function keySubtitle(provider: string): string {
  return pc(metaOf(provider).keySubtitle ?? "Stored encrypted on this Mac.");
}

/** The credential field: a password box with Show, Save and Remove. Only whether a key exists is ever read back. */
function CredentialRow({ id, provider, kind }: { id: string; provider: string; kind: CredentialKind }) {
  const [saved, setSaved] = useState(false);
  const [draft, setDraft] = useState("");
  const [revealed, setRevealed] = useState(false);

  useEffect(() => {
    setDraft("");
    setRevealed(false);
    invoke<boolean>("has_secret", { id }).then(setSaved).catch(() => {});
  }, [id]);

  const save = async () => {
    if (!draft.trim()) return;
    await invoke("set_secret", { id, value: draft.trim() });
    setDraft("");
    setSaved(await invoke<boolean>("has_secret", { id }));
  };
  const remove = async () => {
    await invoke("set_secret", { id, value: null });
    setSaved(false);
  };

  const title = credentialTitle(kind);
  return (
    <Row title={title} subtitle={saved ? `${t("Saved")}. ${keySubtitle(provider)}` : keySubtitle(provider)} wrap>
      <input
        id="account-credential"
        className="text-field"
        type={revealed ? "text" : "password"}
        aria-label={title}
        value={draft}
        placeholder={saved ? "••••••••••••" : ""}
        spellCheck={false}
        autoComplete="off"
        onChange={(e) => setDraft(e.target.value)}
        onKeyDown={(e) => { if (e.key === "Enter") void save(); }}
      />
      <Button onClick={() => setRevealed((r) => !r)}>{revealed ? t("Hide") : t("Show")}</Button>
      <Button disabled={!draft.trim()} onClick={() => void save()}>{t("Save")}</Button>
      <Button disabled={!saved} onClick={() => void remove()}>{t("Remove")}</Button>
    </Row>
  );
}

/** Names the browser about to be opened (upstream `browserHint`, without the keychain warning Windows has no use for). */
function browserHint(chosen: BrowserId | null, present: BrowserId[]): string {
  if (chosen) return t("Only %@.", browserName[chosen]);
  const first = present[0];
  return first ? t("Starts with %@.", browserName[first]) : t("Finds it in the browser you signed in with.");
}

/** Which browser a session is read from, and a button to go and read it. */
function BrowserRow({ id, kind, settings }: { id: string; kind: CredentialKind; settings: AppSettings }) {
  const [installed, setInstalled] = useState<BrowserId[]>([]);
  useEffect(() => {
    invoke<BrowserId[]>("installed_browsers").then(setInstalled).catch(() => {});
  }, []);
  // Only what is actually installed: a browser that isn't there is a choice that can only fail. A browser-storage
  // credential lives in a Chromium profile, which Firefox does not have.
  const present = kind === "browserSession" ? installed.filter((b) => chromium.includes(b)) : installed;
  const chosen = (settings.sessionBrowsers[id] as BrowserId | undefined) ?? null;
  // A browser chosen earlier that has since been removed is still shown, so the row says what is stored.
  const options = [...new Set([...(chosen ? [chosen] : []), ...present])];

  return (
    <Row title={t("Read from browser")} subtitle={browserHint(chosen, present)} wrap>
      <Select<string>
        label={t("Read from browser")}
        value={chosen ?? ""}
        options={[{ value: "", label: t("Automatic") }, ...options.map((b) => ({ value: b, label: browserName[b] }))]}
        onChange={(v) => void setSessionBrowser(settings, id, v || null)}
      />
      {/* TODO(opus): reading the session: Firefox's cookie database directly, Chromium sites through an embedded
          WebView2 sign-in window (docs/SPEC.md §2). Until then the key field above takes the pasted value. */}
      <Button disabled onClick={() => {}}>{t("Read")}</Button>
    </Row>
  );
}
