import '@testing-library/jest-dom/vitest';
import { cleanup } from '@testing-library/react';
import { afterEach, vi } from 'vitest';

/**
 * jsdom 里没有真实终端渲染环境（布局 / canvas），xterm 的 `open()` 无法工作。
 * 单元测试关心的是**调用顺序与参数**（见 TerminalPage.test.tsx），
 * 因此这里给出全局轻量替身；需要断言顺序的测试会自行 mock 覆盖。
 */
vi.mock('@xterm/xterm', () => ({
  Terminal: class {
    cols = 80;
    rows = 24;
    open() {}
    loadAddon() {}
    onData() {
      return { dispose() {} };
    }
    onBinary() {
      return { dispose() {} };
    }
    write() {}
    dispose() {}
  },
}));

vi.mock('@xterm/addon-fit', () => ({
  FitAddon: class {
    fit() {}
  },
}));

afterEach(() => {
  cleanup();
});
