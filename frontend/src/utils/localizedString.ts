import type { LocalizedString } from "@/types/api";

export type LocalizedStringMode = "plain" | "localized";

export interface LocalizedEntry {
  language: string;
  text: string;
}

export function isLocalizedMap(
  value: LocalizedString,
): value is Record<string, string> {
  return typeof value === "object" && value !== null;
}

export function localizedStringToMode(
  value?: LocalizedString,
): LocalizedStringMode {
  if (!value) {
    return "plain";
  }
  return isLocalizedMap(value) ? "localized" : "plain";
}

export function localizedMapToEntries(
  map: Record<string, string>,
): LocalizedEntry[] {
  return Object.entries(map).map(([language, text]) => ({ language, text }));
}

export function entriesToLocalizedMap(
  entries: LocalizedEntry[],
): Record<string, string> | undefined {
  const map: Record<string, string> = {};
  for (const entry of entries) {
    const language = entry.language.trim();
    const text = entry.text.trim();
    if (language && text) {
      map[language] = text;
    }
  }
  return Object.keys(map).length > 0 ? map : undefined;
}

export function defaultLocalizedEntries(seed = ""): LocalizedEntry[] {
  return [
    { language: "en", text: seed },
    { language: "ru", text: "" },
  ];
}

export function formatLocalizedStringDisplay(value?: LocalizedString): string {
  if (!value) {
    return "";
  }
  if (typeof value === "string") {
    return value;
  }
  return Object.entries(value)
    .map(([language, text]) => `${language}: ${text}`)
    .join(" · ");
}
