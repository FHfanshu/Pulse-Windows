// Rewrites macOS-specific wording in the locale VALUES to Windows equivalents.
// Keys are never touched (code looks them up by upstream's English string).
// Idempotent: running it twice changes nothing the second time.
// Usage: node scripts/locale-sweep.mjs [locales-dir]   (default: ../locales next to this script)
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const here = path.dirname(fileURLToPath(import.meta.url));
const localesDir = process.argv[2] ?? path.join(here, "..", "locales");
const LANGS = ["en", "zh-Hans", "zh-Hant", "ja", "ko"];

// Rules run in order, per language. Specific phrases come before the generic words.
// Common rules first (apply to every language).
const COMMON = [
  [/⌘-Tab/g, "Alt+Tab"],
  [/⌘/g, "Ctrl"],
  [/⌥/g, "Alt"],
  [/⌃/g, "Ctrl"],
];

const RULES = {
  en: [
    [/System Settings › General › Login Items/g, "Windows Settings › Apps › Startup"],
    [/System Settings › Notifications/g, "Windows Settings › System › Notifications"],
    [/Use macOS settings or a proxy/g, "Use Windows proxy settings or a proxy"],
    [/\bmenu bar icon\b/g, "tray icon"],
    [/\bMenu bar\b/g, "System tray"],
    [/\bmenu bar\b/g, "system tray"],
    [/\bDock icon\b/g, "taskbar icon"],
    [/\bDock\b/g, "taskbar"],
    [/\bFinder\b/g, "File Explorer"],
    [/\b(?:the|your) keychain\b/g, "Credential Manager access"],
    [/\bKeychain\b/g, "Credential Manager"],
    [/\bkeychain\b/g, "Credential Manager"],
    [/\bmacOS\b/g, "Windows"],
    [/\bSystem Settings\b/g, "Windows Settings"],
    [/\bthis Mac\b/g, "this PC"],
    [/\bMac\b/g, "PC"],
  ],
  "zh-Hans": [
    [/系统设置 › 通用 › 登录项/g, "Windows 设置 › 应用 › 启动"],
    [/系统设置 › 通知/g, "Windows 设置 › 系统 › 通知"],
    [/使用 macOS 系统设置/g, "使用 Windows 设置"],
    [/菜单栏图标/g, "托盘图标"],
    [/菜单栏/g, "系统托盘"],
    [/Dock 图标/g, "任务栏图标"],
    [/Dock/g, "任务栏"],
    [/访达/g, "文件资源管理器"],
    [/钥匙串/g, "凭据管理器"],
    [/系统设置/g, "Windows 设置"],
    [/这台 ?Mac/g, "这台电脑"],
    [/本 ?Mac/g, "本机"],
    [/macOS/g, "Windows"],
    [/\bMac\b/g, "电脑"],
    [/(电脑|本机|系统托盘|任务栏) (?=[一-鿿])/g, "$1"],
    [/([一-鿿]) (?=电脑|本机|系统托盘|任务栏)/g, "$1"],
  ],
  "zh-Hant": [
    [/(?:系統設定|Windows 設定) › 一般 › 登[入錄]項目/g, "Windows 設定 › 應用程式 › 啟動"],
    [/系統設定 › 通知/g, "Windows 設定 › 系統 › 通知"],
    [/使用 macOS 系統設定/g, "使用 Windows 設定"],
    [/選單列圖示/g, "系統匣圖示"],
    [/選單列/g, "系統匣"],
    [/Dock 圖示/g, "工作列圖示"],
    [/Dock/g, "工作列"],
    [/訪達|Finder/g, "檔案總管"],
    [/鑰匙圈/g, "認證管理員"],
    [/系統設定/g, "Windows 設定"],
    [/這台 ?Mac/g, "這台電腦"],
    [/本 ?Mac/g, "本機"],
    [/macOS/g, "Windows"],
    [/\bMac\b/g, "電腦"],
    [/(電腦|本機|系統匣|工作列) (?=[一-鿿])/g, "$1"],
    [/([一-鿿]) (?=電腦|本機|系統匣|工作列)/g, "$1"],
  ],
  ja: [
    [/システム設定 › 一般 › ログイン項目/g, "Windows の設定 › アプリ › スタートアップ"],
    [/システム設定 › 通知/g, "Windows の設定 › システム › 通知"],
    [/メニューバーアイコン/g, "トレイアイコン"],
    [/メニューバー/g, "タスクトレイ"],
    [/Dock アイコン/g, "タスクバーアイコン"],
    [/Dock/g, "タスクバー"],
    [/Finder/g, "エクスプローラー"],
    [/キーチェーン/g, "資格情報マネージャー"],
    [/macOS のシステム設定/g, "Windows の設定"],
    [/システム設定/g, "Windows の設定"],
    [/Windows のWindows の設定/g, "Windows の設定"],
    [/(タスクバー|タスクトレイ|エクスプローラー) (?=[ぁ-んァ-ヶ一-龥])/g, "$1"],
    [/この ?Mac/g, "この PC"],
    [/macOS/g, "Windows"],
    [/\bMac\b/g, "PC"],
  ],
  ko: [
    [/시스템 설정 › 일반 › 로그인 항목/g, "Windows 설정 › 앱 › 시작 프로그램"],
    [/시스템 설정 › 알림/g, "Windows 설정 › 시스템 › 알림"],
    [/메뉴 막대 아이콘/g, "트레이 아이콘"],
    [/메뉴 막대/g, "시스템 트레이"],
    [/Dock 아이콘/g, "작업 표시줄 아이콘"],
    [/Dock/g, "작업 표시줄"],
    [/Finder/g, "파일 탐색기"],
    [/키체인/g, "자격 증명 관리자"],
    [/macOS 시스템 설정/g, "Windows 설정"],
    [/시스템 설정/g, "Windows 설정"],
    [/Windows Windows 설정/g, "Windows 설정"],
    [/이 ?Mac/g, "이 PC"],
    [/macOS/g, "Windows"],
    [/\bMac\b/g, "PC"],
  ],
};

// Whole-value overrides where a regex cannot produce good wording. Keyed by upstream English key.
const OVERRIDES = {
  "Add ⌘, ⌥ or ⌃": {
    en: "Add Ctrl, Alt or Win",
    "zh-Hans": "需要 Ctrl、Alt 或 Win",
    "zh-Hant": "需要 Ctrl、Alt 或 Win",
    ja: "Ctrl、Alt、Win のいずれかを追加",
    ko: "Ctrl, Alt 또는 Win 추가",
  },
};

function applyRules(value, lang) {
  let v = value;
  for (const [re, rep] of [...COMMON, ...RULES[lang]]) v = v.replace(re, rep);
  return v;
}

for (const lang of LANGS) {
  const file = path.join(localesDir, `${lang}.json`);
  const data = JSON.parse(fs.readFileSync(file, "utf8"));
  let changed = 0;
  for (const key of Object.keys(data)) {
    const before = data[key];
    let after = OVERRIDES[key]?.[lang] ?? applyRules(before, lang);
    if (after !== before) {
      data[key] = after;
      changed++;
    }
  }
  fs.writeFileSync(file, JSON.stringify(data, null, 1) + "\n");
  console.log(`${lang}: ${changed} values changed`);
}
