import { useEffect, useState, type ReactNode } from "react";

export function Switch({ checked, onChange, disabled, label }: { checked: boolean; onChange: (v: boolean) => void; disabled?: boolean; label: string }) {
  return (
    <button
      type="button"
      role="switch"
      className="switch"
      aria-checked={checked}
      aria-label={label}
      disabled={disabled}
      onClick={() => onChange(!checked)}
    />
  );
}

export interface Option<T> { value: T; label: string }

/** Segmented control with a blue selected segment. */
export function Segmented<T extends string | number>({ value, options, onChange, disabled, label }: {
  value: T;
  options: Option<T>[];
  onChange: (v: T) => void;
  disabled?: boolean;
  label: string;
}) {
  return (
    <div className="segmented" role="radiogroup" aria-label={label} data-disabled={disabled ? "true" : undefined}>
      {options.map((o) => (
        <button key={String(o.value)} type="button" role="radio" aria-checked={o.value === value} disabled={disabled} onClick={() => onChange(o.value)}>
          {o.label}
        </button>
      ))}
    </div>
  );
}

/** Popup menu drawn as a styled native select. */
export function Select<T extends string | number>({ value, options, onChange, disabled, label }: {
  value: T;
  options: Option<T>[];
  onChange: (v: T) => void;
  disabled?: boolean;
  label: string;
}) {
  return (
    <span className="select">
      <select
        aria-label={label}
        value={String(value)}
        disabled={disabled}
        onChange={(e) => {
          const picked = options.find((o) => String(o.value) === e.target.value);
          if (picked) onChange(picked.value);
        }}
      >
        {options.map((o) => (
          <option key={String(o.value)} value={String(o.value)}>{o.label}</option>
        ))}
      </select>
    </span>
  );
}

export function Slider({ value, onChange, disabled, label }: { value: number; onChange: (v: number) => void; disabled?: boolean; label: string }) {
  return (
    <input
      type="range"
      className="slider"
      aria-label={label}
      min={0}
      max={1}
      step={0.01}
      value={value}
      disabled={disabled}
      style={{ ["--fill" as string]: `${value * 100}%` }}
      onChange={(e) => onChange(Number(e.target.value))}
    />
  );
}

export function Button({ children, onClick, disabled, label }: { children: ReactNode; onClick: () => void; disabled?: boolean; label?: string }) {
  return (
    <button type="button" className="btn" aria-label={label} disabled={disabled} onClick={onClick}>
      {children}
    </button>
  );
}

/** Text entry that holds its own draft and commits on blur or Return. */
export function TextField({ value, onCommit, placeholder, type = "text", label, disabled, onDraft }: {
  value: string;
  onCommit: (v: string) => void;
  onDraft?: (v: string) => void;
  placeholder?: string;
  type?: "text" | "password";
  label: string;
  disabled?: boolean;
}) {
  const [draft, setDraft] = useState(value);
  useEffect(() => setDraft(value), [value]);
  return (
    <input
      className="text-field"
      type={type}
      aria-label={label}
      value={draft}
      placeholder={placeholder}
      disabled={disabled}
      spellCheck={false}
      autoComplete="off"
      onChange={(e) => { setDraft(e.target.value); onDraft?.(e.target.value); }}
      onBlur={() => onCommit(draft)}
      onKeyDown={(e) => { if (e.key === "Enter") onCommit(draft); }}
    />
  );
}
