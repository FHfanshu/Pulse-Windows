// Localization with upstream's keys: the English source string is the key, and
// `%@` placeholders are filled in order (upstream `String.localized`).
import en from "@locales/en.json";
import zhHans from "@locales/zh-Hans.json";
import zhHant from "@locales/zh-Hant.json";
import ja from "@locales/ja.json";
import ko from "@locales/ko.json";

type Table = Record<string, string>;
const tables: Record<string, Table> = { en, "zh-Hans": zhHans, "zh-Hant": zhHant, ja, ko };

export type Language = "system" | keyof typeof tables;

function systemLanguage(): string {
  const tag = (navigator.languages?.[0] ?? navigator.language ?? "en").toLowerCase();
  if (tag.startsWith("zh")) return /tw|hk|mo|hant/.test(tag) ? "zh-Hant" : "zh-Hans";
  if (tag.startsWith("ja")) return "ja";
  if (tag.startsWith("ko")) return "ko";
  return "en";
}

let current = systemLanguage();

export function setLanguage(lang: Language) {
  current = lang === "system" ? systemLanguage() : lang;
}

export function locale(): string {
  return current;
}

/** `t("Resets %@", time)` — key is upstream's English format string. */
export function t(key: string, ...args: (string | number)[]): string {
  const format = tables[current]?.[key] ?? tables.en[key] ?? key;
  let i = 0;
  return format.replace(/%(\d+\$)?@/g, (_, pos?: string) => {
    const index = pos ? parseInt(pos, 10) - 1 : i++;
    return String(args[index] ?? "");
  });
}
