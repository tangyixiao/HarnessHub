# Linux 打包 + CI 发布流水线（A4）计划草案

- Date: 2026-10-24
- 阶段：[ROADMAP 阶段 4（A4）](../ROADMAP.md)「Linux 打包 + CI 发布流水线」
  - 目标：Linux 可安装运行（v0.1 gate）；PR CI 稳定。
  - 范围：AppImage/deb 构建、GitHub Actions artifact、E2E 冒烟矩阵。
  - 验收：Linux 安装包 + 冷启动冒烟证据。
- 本文件是 plan 草案（调研阶段产出）；实施前按 AGENTS.md 规则可再补 spec，
  或者把本文件 §2/§3 的结论直接升格为 spec。
- 约束：本文档**只做调研与计划**，不改代码、不改 src-tauri/（Rust 侧另有并行工作）；
  实施时新建 `.github/workflows/release.yml` 属于新文件，不算修改已有 workflow。

## 0. 证据基线

本文所有结论来自以下文件（2026-10-24 读于 `codex/git-snapshot-capture` 分支）：

| 文件                                                                    | 用途                                             |
| ----------------------------------------------------------------------- | ------------------------------------------------ |
| `.github/workflows/ci.yml`（62 行，仓库唯一 workflow）                  | §1 CI 现状                                       |
| `package.json`                                                          | scripts、版本 0.1.0-alpha.4、pnpm@11.0.9         |
| `src-tauri/tauri.conf.json`                                             | bundle.targets、icon、productName                |
| `src-tauri/Cargo.toml`                                                  | 依赖（rusqlite bundled、windows-sys 仅 Windows） |
| `docs/releases/v0.1.0-alpha.4.md`                                       | alpha.4 发布形态与证据                           |
| `docs/plans/2026-10-23-alpha4-release.md`                               | 发布流程细节、CI stalled 事故记录                |
| `docs/ROADMAP.md`（A4 一节）                                            | 阶段目标与验收                                   |
| `docs/CONTEXT.md`                                                       | 技术栈、首选平台（Windows、Linux；macOS 后续）   |
| `tests/e2e/README.md`（git HEAD）                                       | 冷启动冒烟证据格式、早期「Linux 未验证」记录     |
| `src-tauri/src/plugins/discovery.rs`、`manifest.rs`                     | 插件宿主对 `python` 的 PATH 依赖                 |
| Tauri v2 官方文档（AppImage 分发页，v2.tauri.app/distribute/appimage/） | glibc 基座、CI 建议                              |
| GitHub runner-images issue #14254                                       | ubuntu-22.04 镜像弃用时间表                      |

---

## 1. 当前 CI 的真实状态

### 1.1 有哪些 workflow、跑了什么

仓库只有 **一个** workflow：`.github/workflows/ci.yml`，`name: CI`，
触发条件是 `push`（仅 `main`）与 `pull_request`（ci.yml:3-6）。三个 job，全部跑在 `ubuntu-latest`：

| Job                                                   | 内容                                                                                                                                                                                                                                                                          | 位置         |
| ----------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------ |
| `frontend`「前端（lint / typecheck / test / build）」 | `pnpm/action-setup@v4`（不写 `version`，以 `packageManager` 为唯一事实来源，见 ci.yml:17-21 注释）→ `setup-node@v4`（node 22 + pnpm 缓存）→ `pnpm install --frozen-lockfile` → `lint` / `typecheck` / `test` / `build`                                                        | ci.yml:12-30 |
| `rust`「Rust Control Plane（fmt / clippy / test）」   | `dtolnay/rust-toolchain@stable` + `Swatinem/rust-cache@v2`（`workspaces: src-tauri`）→ **apt 安装 Tauri Linux 依赖**（libwebkit2gtk-4.1-dev、libappindicator3-dev、librsvg2-dev、patchelf，ci.yml:44-47）→ `cargo fmt --check` / `clippy -D warnings` / `test -- --nocapture` | ci.yml:32-53 |
| `python`「Python sidecar（unittest）」                | `astral-sh/setup-uv@v5` → `uv run ... unittest discover`（未配置 uv 缓存）                                                                                                                                                                                                    | ci.yml:55-61 |

Linux 依赖安装这步**已经在真实 runner 上验证过**：PR #17（run 39955830641）attempt 2 通过；
attempt 1 在安装系统依赖时被取消（`docs/plans/2026-10-23-alpha4-release.md:56`、
`docs/releases/v0.1.0-alpha.4.md:40`）。也就是说：**Linux 上「编译 + 测试」链路已通，
「打包 + 发布」链路完全不存在**。

### 1.2 Linux 上缺什么

1. **没有任何打包 job**。`tauri build` 从未在 CI 跑过；alpha.4 的 Windows NSIS 是
   **本地**构建后手工上传 Release（alpha4-release plan:73-79 记录了本地 NSIS 下载
   超时与重试全过程），CI 只承担检查职责。
2. **没有 release workflow**（无 tag 触发）。alpha.4 发布是手工流程：
   本地构建 → SHA-256 → 建 tag → 建 GitHub Release → 上传 2 个资产（installer + SHA256SUMS）。
3. **`pnpm format:check` 不在任何 job 里**。它是 AGENTS.md「完成前」九项门之一
   （`package.json:18` 有该脚本），但 ci.yml 的 frontend job 只跑了 lint/typecheck/test/build。
4. **没有 E2E 冒烟矩阵**。Windows 冷启动冒烟（隔离 identifier + 隐藏窗口 + SQLite 校验，
   `docs/releases/v0.1.0-alpha.4.md:54`）是本地脚本；Linux 没有对应物。
   `tests/e2e/README.md` 早期就记录过「Linux 平台：本机未验证」。
5. **没有 job 级 `timeout-minutes`、没有 `concurrency` 组**。stalled 事故的第一现场就是
   依赖安装步骤，目前靠人肉取消重跑。
6. 小项：python job 无 uv 缓存；rust job 的 apt 依赖清单与将来的打包 job 重复（可抽 composite action，非必须）。

---

## 2. Tauri 2 Linux 打包产物选择建议

`tauri.conf.json:30` 已配置 `"targets": "all"`——Linux 上 `tauri build` 默认会产出
**deb + rpm + AppImage** 三种，无需改配置即可全量构建。

| 产物         | 建议                  | 理由                                                                                                                                                                                                                                                                        |
| ------------ | --------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| **deb**      | **主交付**（A4 必做） | Ubuntu/Debian 是桌面开发环境主流；系统级安装/卸载、依赖由 Tauri 自动声明（含 webkit2gtk-4.1）；ROADMAP A4 明确点名「AppImage/deb 构建」。                                                                                                                                   |
| **AppImage** | **次交付**（A4 必做） | 免 root、单文件分发，适合先试后装；ROADMAP 点名。**注意**：Tauri 官方文档要求构建基座提供 WebKitGTK 4.1 包，且明确「用你打算支持的最老基座系统来构建」（glibc 符号如 GLIBC_2.33 会在老系统上缺失）——AppImage 对 Tauri 而言**不是零依赖绿色包**，宿主仍需 WebKitGTK 运行库。 |
| **rpm**      | 顺带保留，不单列任务  | Tauri 2 原生支持，`targets: "all"` 顺带产出，覆盖 Fedora 用户；不写额外验证步骤（A4 验收只要求「Linux 安装包」，以 deb 为证据）。                                                                                                                                           |
| macOS        | 范围外                | `docs/CONTEXT.md:30`：「macOS 后续正式支持」。release workflow 留 matrix 扩展位即可。                                                                                                                                                                                       |

产物命名注意：`productName: "Harness Hub"`（tauri.conf.json:3）带空格，alpha.4 时
Windows 安装包就因文件名空格在上传前改名为无空格资产名（alpha4-release plan:76）。
Linux 的 deb/AppImage 文件名同理，实施时按同样先例在**上传前重命名**（或届时验证
Tauri v2 的 `mainBinaryName` 能否直接解决，作为实现细节）。

---

## 3. 建议的 GitHub Actions 增改要点

### 3.1 PR CI（改 `.github/workflows/ci.yml`，小改）

1. frontend job 的 `pnpm build` 之后补一行 `pnpm format:check`（补齐 AGENTS.md 门）。
2. 每个 job 加 `timeout-minutes: 30` 左右——stalled 依赖安装应快速失败而不是挂到 6 小时上限。
3. 顶层加并发取消，避免连环 push 堆积排队（也是 stalled 观感的来源之一）：

```yaml
concurrency:
  group: ci-${{ github.ref }}
  cancel-in-progress: true
```

4. apt 依赖安装步骤保持现状（拆分 `update` 与 `install` 两步便于定位卡点），等 release
   workflow 稳定后可抽 composite action 复用，非必须。

### 3.2 新增 release workflow（`.github/workflows/release.yml`）草图

```yaml
name: Release

on:
  push:
    tags: ['v*'] # v0.1.x / v0.x.y[-pre]
  workflow_dispatch: # 手动试跑

concurrency:
  group: release-${{ github.ref }}
  cancel-in-progress: false

jobs:
  linux:
    # glibc 兼容面：Tauri 文档要求「最老基座」。ubuntu-latest 已是 24.04；
    # 22.04 镜像已进入弃用流程（brownouts 预计 2027-03 起），见 §4 风险 1。
    runs-on: ubuntu-22.04
    timeout-minutes: 60
    outputs:
      version: ${{ steps.ver.outputs.version }}
    steps:
      - uses: actions/checkout@v4
      - uses: pnpm/action-setup@v4 # 版本仍以 packageManager 为准，不写 version
      - uses: actions/setup-node@v4
        with: { node-version: 22, cache: pnpm }
      - uses: dtolnay/rust-toolchain@stable
      - uses: Swatinem/rust-cache@v2
        with:
          workspaces: src-tauri
      - name: 安装 Tauri Linux 依赖
        run: |
          sudo apt-get update
          sudo apt-get install -y --no-install-recommends \
            libwebkit2gtk-4.1-dev libappindicator3-dev librsvg2-dev patchelf
      - run: pnpm install --frozen-lockfile
      - name: 校验版本一致性
        id: ver
        run: |
          # alpha.4 的先例：package.json / Cargo.toml / Cargo.lock / tauri.conf.json 四处同步
          # （docs/plans/2026-10-23-alpha4-release.md「Version synchronization」）
          TAG="${GITHUB_REF_NAME#v}"
          test "$(node -p "require('./package.json').version")" = "$TAG"
          test "$(node -p "require('./src-tauri/tauri.conf.json').version")" = "$TAG"
          grep -q "^version = \"$TAG\"" src-tauri/Cargo.toml
          echo "version=$TAG" >> "$GITHUB_OUTPUT"
      - uses: tauri-apps/tauri-action@v0
        env:
          GITHUB_TOKEN: ${{ secrets.GITHUB_TOKEN }}
          APPIMAGE_EXTRACT_AND_RUN: '1' # runner 无 FUSE 时 AppImage 打包兜底
        with:
          tagName: ${{ github.ref_name }}
          releaseName: 'Harness Hub ${{ github.ref_name }}'
          releaseDraft: false
          prerelease: true # 与 alpha.4「显式 pre-release」一致
          args: --bundles deb,appimage # rpm 想要就改 deb,appimage,rpm
      - name: 重命名资产（去空格）
        run: |
          # "Harness Hub_0.1.0-alpha.4_amd64.deb" → "Harness-Hub_...deb"（alpha.4 先例）
          mkdir -p renamed
          for f in target/release/bundle/deb/*.deb target/release/bundle/appimage/*.AppImage; do
            cp "$f" "renamed/$(basename "$f" | tr ' ' '-')"
          done
      - uses: actions/upload-artifact@v4
        with:
          name: linux-bundles
          path: renamed/*
```

说明：

- 选 `tauri-action` 而不是手写 `pnpm tauri build`：它同时处理「构建 → 建/更新 Release →
  上传资产」，并内置 AppImage/FUSE 的环境处理；`workflow_dispatch` 允许不发版先试跑。
- **先 artifact 后 release**：上面草图两者都做；如果想先人工验收再发布，可把 tauri-action
  的 `tagName` 去掉（只出 artifact），验收后第二个 job 再上传。v0.1 阶段建议保留自动建
  pre-release（与 alpha.4 的 pre-release 纪律一致）。
- `Cargo.lock` 与 `pnpm-lock.yaml` 都已在仓库中，`--frozen-lockfile` 照旧。

### 3.3 Linux 冷启动冒烟（E2E 冒烟矩阵第一格）

复刻 Windows 冒烟的思路（`docs/releases/v0.1.0-alpha.4.md:54` 的证据格式），
作为 release workflow 的独立 job（`needs: linux`）：

1. 下载 artifact 中的产物。
2. **无 bundle 冷启动**：用 release 构建的裸二进制 `target/release/harness-hub`，
   配一份 tauri config override（隔离 identifier 如 `dev.harnesshub.smoke.<date>` +
   隐藏窗口），在 `xvfb-run -a` 下启动；Linux 侧用 `XDG_DATA_HOME` 指到隔离目录实现
   app-data 隔离（Windows 是靠 identifier override 绕过 Known Folder，Linux 用 XDG 更干净）。
3. 等 20–30 秒后用 Python 标准库 `sqlite3` 只读检查
   `$XDG_DATA_HOME/dev.harnesshub.desktop/harness-hub.sqlite3`：
   `schemaVersion` 正确、`harnesses=5, sessions=0`（与 alpha.4 冒烟同一断言形状），
   进程存活、随后干净退出，`exit=0`。
4. **deb 安装冒烟**（可选增强）：`docker run --rm -v deb:/deb ubuntu:24.04`
   里 `apt-get update && apt-get install -y /deb/*.deb`，验证包元数据与依赖声明可解析
   （容器内不起 GUI，只验证「装得上」）。

验收证据（ROADMAP A4）= release run 的冒烟 job 日志 + 冒烟 SQLite 校验输出，
写进对应 `docs/releases/` 文档，沿用「真实输出而非应该可以」的记录纪律。

---

## 4. 风险与开放问题

1. **runner 基座 vs glibc 兼容面（最重要）**。Tauri 文档明确要求「用打算支持的
   最老基座系统构建」，否则产物在老系统上报 `GLIBC_2.33` 类符号缺失。当前
   `ubuntu-latest` 已是 24.04（glibc 2.39），意味着默认产物只保证 Ubuntu 24.04+；
   而 `ubuntu-22.04` 镜像已进入官方弃用流程（brownouts 预计 2027-03 开始、约 2027-04
   退役，[actions/runner-images#14254](https://github.com/actions/runner-images/issues/14254)）。
   三个选项：**(a)** 短期钉 `ubuntu-22.04`（现在仍可用，最省事，含迁移倒计时）；
   **(b)** 把打包 job 放进 `container: ubuntu:22.04`（不受镜像退役影响，但要自己装
   node/rust/pnpm，维护成本高）；**(c)** 明确只支持 24.04+ 并写进 release notes。
   **推荐 (a)**，并在 A4 实施时把「22.04 退役前迁移到容器构建或宣布 24.04 基线」
   记为后续任务。
2. **依赖安装步骤曾 stalled**。PR #17 run 39955830641 attempt 1 在装系统依赖时被取消，
   attempt 2 才通过（alpha4-release plan:56、releases doc:40）。缓解：job 级
   `timeout-minutes` 快速失败、`concurrency` 取消过期 run、apt update/install 拆分
   便于看日志定位；失败后直接重跑 attempt 即可，暂不做自动重试（保持流水线简单）。
3. **打包工具链的外部下载**。Tauri bundler 在 bundle 阶段要下载外部工具
   （AppImage 走 linuxdeploy；本地 NSIS 下载就超时过一次，alpha4-release plan:73-75）。
   CI 上首次 run 要盯网络抖动；`APPIMAGE_EXTRACT_AND_RUN=1` 兜底 runner 无 FUSE。
   若反复抖动再考虑缓存 `~/.cache/tauri`，先不过度设计。
4. **`python` 在 Debian/Ubuntu 默认不存在**（只有 `python3`，除非装
   `python-is-python3`）。插件宿主的 manifest 是 `command = ["python", ...]`
   （`src-tauri/src/plugins/discovery.rs:138`、`src-tauri/src/plugins/manifest.rs:230`），
   supervisor 直接 `Command::new("python")`（`src-tauri/src/plugins/supervisor.rs:189`）。
   这意味着 Linux 用户的插件功能开箱即坏。**开放问题**：运行时按 `python` → `python3`
   顺序解析，还是 release notes 写明依赖 `python-is-python3`？涉及 Rust 代码，
   本计划不改（Rust 侧另有并行工作），A4 实施前需与 Rust 侧协调归属。
   冒烟机（runner）上先装 `python-is-python3` 不影响本判定。
5. **发布语义**：release workflow 自动建 GitHub Release + 上传资产，但 alpha.4 流程里
   「重开 Release 核对 metadata」是人工步骤（alpha4-release plan:87）。v0.1 正式版（A5）
   是否仍要求人工核验后才公开？建议：workflow 产出 **draft release**，人工核对后
   publish——既自动化又保留证据纪律。（草图里 `releaseDraft: false` 对应 alpha.4
   的直接发布习惯，实施时可改为 `true`，取决于 §4.6 的决定。）
6. **开放问题（小项）**：
   - `mainBinaryName` 是否需要显式设为 `harness-hub`，以避免二进制/deb 文件名带空格
     （alpha.4 先例是事后改名；实现时验证 Tauri v2 实际产物名再定）。
   - `bundle.icon`（tauri.conf.json:34-40）只列了 32/128/256 的 png + icns/ico；
     `icons/icon.png`、`source.png` 未列入。deb/AppImage 取最大 png，实现时确认
     bundler 无警告、AppImage 图标清晰即可，不阻塞。
   - python job 是否加 uv 缓存（`setup-uv` 支持 `enable-cache`），收益小，顺手项。
   - rpm 若保留 `targets: "all"` 会自动产出，但 A4 验收不覆盖它；要么显式
     `--bundles deb,appimage` 控制范围，要么接受「产出但不验收」并如实记录。

## 5. 建议的 Task 拆分（供排期参考）

1. `test:`/`chore:` ci.yml 小改（format:check、timeout、concurrency）——独立小 PR，先行。
2. release.yml + Linux 打包（deb/AppImage artifact，workflow_dispatch 试跑）。
3. Linux 冷启动冒烟 job + 证据落 `docs/releases/`。
4. 打 tag 走一次全流程端到端（预发 pre-release），回填 A4 验收证据。
