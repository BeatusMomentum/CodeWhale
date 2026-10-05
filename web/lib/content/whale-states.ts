import type { LocalizedText } from "./vocabulary";
import { pickText } from "@/lib/i18n/dictionaries";

/**
 * The words under the homepage whale. Every state reads without motion or
 * colour: a mark and a word (codewhale-design DIRECTION.md, Craft). The
 * performance is an illustration of a session's phases, not live telemetry.
 */
export const WHALE_STATE_TEXT = {
  rest: { en: "Ready", zh: "就绪" },
  listen: { en: "Reading your request", zh: "正在阅读你的请求" },
  think: { en: "Planning", zh: "正在规划" },
  read: { en: "Reading files", zh: "正在读取文件" },
  write: { en: "Editing", zh: "正在编辑" },
  run: { en: "Running tests", zh: "正在运行测试" },
  done: { en: "Done", zh: "完成" },
  pod: { en: "Working with three agents", zh: "三个智能体协作中" },
  connect: { en: "Connecting a provider", zh: "正在连接提供商" },
  busy: { en: "Working", zh: "工作中" },
} satisfies Record<string, LocalizedText>;

export type WhaleStateKey = keyof typeof WHALE_STATE_TEXT;

export function whaleStateLabels(locale: string): Record<WhaleStateKey, string> {
  return Object.fromEntries(
    Object.entries(WHALE_STATE_TEXT).map(([key, text]) => [key, pickText(text, locale)]),
  ) as Record<WhaleStateKey, string>;
}
