// Ported from upstream Settings/ShortcutField.swift and the key naming in App/GlobalShortcut.swift.
// The control that records a global shortcut: a key-cap sized button that shows the current
// combination and takes the next one pressed after a click. Accelerators use the syntax of
// Tauri's global-shortcut plugin ("Ctrl+Alt+KeyP"); the field shows them as "Ctrl+Alt+P".
import { useEffect, useRef, useState } from "react";
import { t } from "../shared/i18n";

export interface GlobalShortcut { accelerator: string }

/** Ctrl, Alt or Win must be part of it: a bare letter would steal typing everywhere. */
const REQUIRED = ["Ctrl", "Alt", "Super"];
const MODIFIER_CODES = new Set(["ControlLeft", "ControlRight", "AltLeft", "AltRight", "ShiftLeft", "ShiftRight", "MetaLeft", "MetaRight", "OSLeft", "OSRight"]);

function heldModifiers(e: { ctrlKey: boolean; altKey: boolean; shiftKey: boolean; metaKey: boolean }): string[] {
  const out: string[] = [];
  if (e.ctrlKey) out.push("Ctrl");
  if (e.altKey) out.push("Alt");
  if (e.shiftKey) out.push("Shift");
  if (e.metaKey) out.push("Super");
  return out;
}

const keyNames: Record<string, string> = {
  Space: "Space", Enter: "Enter", Tab: "Tab", Backquote: "`", Minus: "-", Equal: "=", BracketLeft: "[", BracketRight: "]",
  Backslash: "\\", Semicolon: ";", Quote: "'", Comma: ",", Period: ".", Slash: "/", ArrowUp: "↑", ArrowDown: "↓",
  ArrowLeft: "←", ArrowRight: "→", Home: "Home", End: "End", PageUp: "PgUp", PageDown: "PgDn", Insert: "Ins",
};

/** "KeyP" → "P", "Digit1" → "1", "Super" → "Win". */
export function displayAccelerator(accelerator: string): string {
  return accelerator
    .split("+")
    .map((part) => {
      if (part === "Super") return "Win";
      if (/^Key[A-Z]$/.test(part)) return part.slice(3);
      if (/^Digit\d$/.test(part)) return part.slice(5);
      if (/^Numpad\d$/.test(part)) return `Num ${part.slice(6)}`;
      return keyNames[part] ?? part;
    })
    .join("+");
}

export function ShortcutField({ shortcut, onChange }: { shortcut: GlobalShortcut | null; onChange: (s: GlobalShortcut | null) => void }) {
  const [recording, setRecording] = useState(false);
  const [held, setHeld] = useState<string[]>([]);
  const [needsModifier, setNeedsModifier] = useState(false);
  const button = useRef<HTMLButtonElement>(null);

  const stop = () => {
    setRecording(false);
    setHeld([]);
    setNeedsModifier(false);
  };

  useEffect(() => {
    if (!recording) return;
    // Every key is swallowed while recording: half a combination must not reach a field, the whole one not a menu.
    const down = (e: KeyboardEvent) => {
      e.preventDefault();
      e.stopPropagation();
      const modifiers = heldModifiers(e);
      if (MODIFIER_CODES.has(e.code)) {
        setHeld(modifiers);
        if (modifiers.some((m) => REQUIRED.includes(m))) setNeedsModifier(false);
        return;
      }
      const bare = modifiers.length === 0;
      // Escape leaves it as it was; Backspace/Delete take it away. Both only bare.
      if (bare && e.code === "Escape") return stop();
      if (bare && (e.code === "Backspace" || e.code === "Delete")) {
        stop();
        return onChange(null);
      }
      if (!modifiers.some((m) => REQUIRED.includes(m))) {
        setNeedsModifier(true);
        return;
      }
      stop();
      onChange({ accelerator: [...modifiers, e.code].join("+") });
    };
    const up = (e: KeyboardEvent) => setHeld(heldModifiers(e));
    // Recording holds the keyboard, so it must not outlive the window's focus.
    const blur = () => stop();
    window.addEventListener("keydown", down, true);
    window.addEventListener("keyup", up, true);
    window.addEventListener("blur", blur);
    return () => {
      window.removeEventListener("keydown", down, true);
      window.removeEventListener("keyup", up, true);
      window.removeEventListener("blur", blur);
    };
  }, [recording]); // eslint-disable-line react-hooks/exhaustive-deps

  const label = recording
    ? needsModifier
      ? t("Add Ctrl or Alt")
      : held.length
        ? displayAccelerator(held.join("+"))
        : t("Press keys…")
    : shortcut
      ? displayAccelerator(shortcut.accelerator)
      : t("Not set");

  return (
    <div className="shortcut-field">
      <button
        ref={button}
        type="button"
        className={`key-cap${recording ? " recording" : ""}`}
        aria-label={t("Record a shortcut")}
        onClick={() => (recording ? stop() : setRecording(true))}
      >
        {label}
      </button>
      <button
        type="button"
        className="key-clear"
        aria-label={t("Clear this shortcut")}
        // Kept in the layout when there is nothing to clear, so the caps don't slide sideways.
        style={{ opacity: shortcut && !recording ? 1 : 0 }}
        disabled={!shortcut || recording}
        onClick={() => onChange(null)}
      >
        <svg width="14" height="14" viewBox="0 0 16 16" aria-hidden>
          <circle cx="8" cy="8" r="7" fill="currentColor" opacity="0.45" />
          <path d="M5.5 5.5l5 5M10.5 5.5l-5 5" stroke="var(--s-card)" strokeWidth="1.6" strokeLinecap="round" />
        </svg>
      </button>
    </div>
  );
}
