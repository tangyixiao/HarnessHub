/**
 * Harness 身份色。
 *
 * 颜色编码规则（docs/specs/2026-10-11-ui-visual-polish-design.md「研究依据」）：
 * 先有文字约定，再谈颜色 —— 身份色只做**次级编码**（名称永远并行显示），
 * 所以用低饱和的同一亮度/彩度、只变色相，避免「彩虹沙拉」。
 *
 * 已知 Harness 的色相是**手工固定**的映射，不是哈希：同一 Harness 在
 * Dashboard / Sessions / Harnesses 三处必须永远同色。未知 id 一律中性灰，
 * 不猜色相。
 */

type Identity = {
  /** 前景（文字 / 图标） */
  fg: string;
  /** 章底 */
  bg: string;
  /** 描边 */
  ring: string;
};

const KNOWN_HUES: Record<string, number> = {
  codex: 220, // 天蓝
  claude: 55, // 陶橙（Anthropic 系）
  gemini: 290, // 紫罗兰（Google 系）
  zcode: 160, // 青绿
  opencode: 130, // 草绿
};

const NEUTRAL: Identity = {
  fg: 'oklch(0.72 0.012 265)',
  bg: 'oklch(0.72 0.012 265 / 0.09)',
  ring: 'oklch(0.72 0.012 265 / 0.22)',
};

export function harnessIdentity(id: string): Identity {
  const hue = KNOWN_HUES[id.trim().toLowerCase()];
  if (hue === undefined) {
    return NEUTRAL;
  }
  return {
    fg: `oklch(0.78 0.115 ${hue})`,
    bg: `oklch(0.78 0.115 ${hue} / 0.13)`,
    ring: `oklch(0.78 0.115 ${hue} / 0.32)`,
  };
}
