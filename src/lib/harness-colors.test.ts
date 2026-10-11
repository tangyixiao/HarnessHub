import { describe, expect, it } from 'vitest';

import { harnessIdentity } from './harness-colors';

describe('harnessIdentity', () => {
  it('已知 Harness 拿到固定身份色（同一 id 永远同色）', () => {
    const KNOWN = ['codex', 'claude', 'gemini', 'zcode', 'opencode', 'codebuddy'];
    for (const id of KNOWN) {
      const first = harnessIdentity(id);
      const second = harnessIdentity(id);
      expect(first).toEqual(second);
      // 已知 Harness 的前景带明显彩度（0.115）
      expect(first.fg).toContain('0.115');
    }
    // 不同 Harness 的色相必须不同
    const hues = KNOWN.map((id) => harnessIdentity(id).fg);
    expect(new Set(hues).size).toBe(6);
  });

  it('未知 Harness 返回中性色（不猜色相）', () => {
    const identity = harnessIdentity('some-new-harness');
    expect(identity).toEqual(harnessIdentity('another-unknown'));
    // 中性色的彩度接近零（0.012），与身份色可区分
    expect(identity.fg).toContain('0.012');
  });

  it('空 id 与大小写变体安全处理', () => {
    expect(harnessIdentity('')).toEqual(harnessIdentity('some-new-harness'));
    expect(harnessIdentity('Codex').fg).toEqual(harnessIdentity('codex').fg);
  });
});
