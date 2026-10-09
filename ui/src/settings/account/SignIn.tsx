// Ported from upstream Settings/ProviderSignInModel.swift (state) and the sign-in rows of
// Settings/AccountsGroup.swift and Settings/AccountConnectionGroup.swift (Copilot's GitHub row).
//
// The state lives in Rust (src-tauri/src/signin_ipc.rs), not in a pane: a device-code sign-in polls for fifteen
// minutes, and the Cancel button, the code and the error have to be where the reader left them when they come
// back. `useSignIn` reads it on mount and follows the `signin-changed` event.
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { useEffect, useState } from "react";
import { t } from "../../shared/i18n";
import { Button } from "../controls";
import { Row } from "../Group";

export interface SignInPrompt {
  userCode: string;
  verificationUrl: string;
  /** The link already carries the code (xAI's does; GitHub's and OpenAI's never do). */
  prefilled: boolean;
}

export interface SignInState {
  /** The provider an added-account sign-in is currently open for. */
  signingIn: string | null;
  signInError: { provider: string; message: string } | null;
  devicePrompt: SignInPrompt | null;
  githubActive: boolean;
  githubPrompt: SignInPrompt | null;
  githubError: string | null;
  githubSignedIn: boolean;
}

const idle: SignInState = {
  signingIn: null,
  signInError: null,
  devicePrompt: null,
  githubActive: false,
  githubPrompt: null,
  githubError: null,
  githubSignedIn: false,
};

export function useSignIn(): SignInState {
  const [state, setState] = useState<SignInState>(idle);
  useEffect(() => {
    let alive = true;
    invoke<SignInState>("signin_state").then((s) => { if (alive) setState(s); }).catch(() => {});
    const un = listen<SignInState>("signin-changed", (e) => setState(e.payload));
    return () => {
      alive = false;
      void un.then((f) => f());
    };
  }, []);
  return state;
}

/** Messages are an upstream English string or the provider's own words; `t()` passes the latter through. */
const say = (message: string) => t(message);

const start = (provider: string, replacing: string | null) =>
  void invoke("signin_start", { provider, replacing }).catch(() => {});

/** While a device-code sign-in waits, the code is the whole interaction: it is typed on the provider's page. */
function CodeRow({ provider, prompt, subtitle }: { provider: string; prompt: SignInPrompt; subtitle: string }) {
  return (
    <Row title={t("Code")} subtitle={subtitle}>
      <span style={{ fontFamily: "ui-monospace, Consolas, monospace", fontSize: 13, fontWeight: 600, userSelect: "text" }}>
        {prompt.userCode}
      </span>
      <Button onClick={() => void invoke("signin_copy_code", { provider })}>{t("Copy")}</Button>
      <Button onClick={() => void invoke("signin_open_page", { provider })}>{t("Open page")}</Button>
    </Row>
  );
}

/**
 * The first rows of the Accounts group for a provider with `multipleAccounts`: "Add another account" (a first
 * account) or "Sign in again…" (an added one), then, while it runs, the device code and any error. One sign-in at a
 * time, and its Cancel, code and error belong to the provider it was started for: a Codex device code shown on the
 * Claude Code pane reads as Claude Code asking for it.
 */
export function AddAccountRows({ id, provider, primary }: { id: string; provider: string; primary: boolean }) {
  const state = useSignIn();
  const mine = state.signingIn === provider;
  const error = state.signInError && state.signInError.provider === provider ? state.signInError : null;
  return (
    <>
      <Row
        title={primary ? t("Add another account") : t("Sign in again…")}
        // The one thing someone should know before they start: whose name is on the page that opens.
        subtitle={t("Opens the provider's own sign-in page.")}
      >
        {mine ? (
          <Button onClick={() => void invoke("signin_cancel", { provider })}>{t("Cancel")}</Button>
        ) : (
          <Button disabled={state.signingIn !== null} onClick={() => start(provider, primary ? null : id)}>
            {t("Sign in…")}
          </Button>
        )}
      </Row>
      {mine && state.devicePrompt && (
        <CodeRow
          provider={provider}
          prompt={state.devicePrompt}
          // OpenAI's own hand-off to a Google account fails with `token_exchange_failed` when the browser has no
          // session, and this row is the only place that says so. Which half is said depends on whether the
          // provider's own link already carries the code.
          subtitle={state.devicePrompt.prefilled
            ? t("Already on the page. Sign in there first if asked, then approve it.")
            : t("Copied. Sign in there first if asked, then paste it.")}
        />
      )}
      {error && <Row title={t("Sign-in")} subtitle={say(error.message)} />}
    </>
  );
}

/**
 * Copilot's GitHub account: a sign-in, not a pasted token. The endpoint would accept the one `gh` holds, but that
 * carries `repo` and `workflow`; this asks for `read:user`. Sign in / Cancel / Sign out, the code row while it
 * waits, and the error as the subtitle.
 */
export function CopilotSignInRows({ id }: { id: string }) {
  const state = useSignIn();
  const subtitle = state.githubError
    ? say(state.githubError)
    : state.githubSignedIn
      ? t("Signed in. Pulse holds a read-only token for this PC.")
      : t("Opens GitHub's own page. Pulse asks to read your profile, nothing else.");
  return (
    <>
      <Row title={t("GitHub account")} subtitle={subtitle}>
        {state.githubActive ? (
          <Button onClick={() => void invoke("signin_cancel", { provider: "copilot" })}>{t("Cancel")}</Button>
        ) : state.githubSignedIn ? (
          <Button onClick={() => void invoke("signin_sign_out", { account: id }).catch(() => {})}>{t("Sign out")}</Button>
        ) : (
          <Button onClick={() => start("copilot", null)}>{t("Sign in…")}</Button>
        )}
      </Row>
      {state.githubPrompt && (
        <CodeRow provider="copilot" prompt={state.githubPrompt} subtitle={t("Copied — paste it on the page that opened.")} />
      )}
    </>
  );
}
