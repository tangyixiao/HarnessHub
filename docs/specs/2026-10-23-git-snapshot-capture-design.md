# Git observer 第一阶段：只读快照采集

- 日期：2026-10-23。
- 来源：`HARNESS_HUB_PROJECT_PLAN.md` §3.1 Git Activity、§20 第 16 步。
- 用户已确认分两阶段，并确认本阶段先实现内部 Rust 采集器、真实临时仓库测试和独立 PR。

## 目标与范围

把 `src-tauri/src/git/` 从占位模块变成可测试的只读仓库快照采集器。
输入是本机目录，输出是该目录所属工作树的 HEAD、路径和文件状态计数。
不读取或保存文件正文、diff、提交消息、远端地址或逐文件路径。

本阶段不落 SQLite、不新增 migration、不接 Session 生命周期、不新增 IPC 或 UI。
第二阶段再定义 Session 起止比较基准、行数差异、提交计数、持久化和历史展示。
因此本阶段完成不能宣称完整的 Git Session Snapshot release gate 已满足。

## 选择

1. **采用：先做独立只读采集器。** 可以真实验证 Git 边界，Session 接入时复用，出错不会影响现有终端。
2. 同时接 Session 和历史页：端到端价值更直接，但把采集、退出时序、schema 和 IPC 绑成一个大改动。
3. 直接引入 Git 库：可以避免子进程，但会增加依赖和行为兼容成本，本阶段不采用。

沿用已有系统 Git、结构化 argv 和 `-C` 方式，不经过 shell，不改进程当前目录。
已有 `project::git_root::GitRootProbe` 是登记项目的单值探测接口；不为本阶段新建第二套 root trait。
检查现有实现后，采集器可集中执行多字段工作树探测；若共享命令执行器，应保持现有项目登记契约并补回归测试。
不为将来可能的远程 runtime、缓存或调度器预建抽象。

## 内部接口与语义

领域接口放在 `git` 模块：

```rust
pub fn capture(path: &Path) -> Result<GitObservation, GitCaptureError>;

pub enum GitObservation {
    NotRepository,
    Repository(GitSnapshot),
}

pub struct GitSnapshot {
    pub worktree_root: PathBuf,
    pub git_dir: PathBuf,
    pub common_dir: PathBuf,
    pub head: Option<String>,
    pub staged_files: u64,
    pub unstaged_files: u64,
    pub untracked_files: u64,
    pub conflicted_files: u64,
}
```

- 仅接受现存目录；不存在或文件输入是 `InvalidPath`，不是“没有改动”。
- 普通非仓库目录返回 `NotRepository`；裸仓库也是 `NotRepository`，因为没有可观察工作树。
- Git 不可执行、权限/配置错误、超时、输出过大和输出损坏是不同错误类别；不得伪装成非仓库。
- 普通仓库和子目录返回同一个绝对工作树根。Linked worktree 返回自己的工作树根和 git_dir，common_dir 指向共享管理目录。
- 已提交仓库的 `head` 是完整十六进制 object ID（支持 SHA-1/SHA-256），detached HEAD 仍正常返回。
- 空仓库的合法 unborn HEAD 返回 `None`；损坏的 HEAD/对象或其他命令失败必须返回错误，不能当 unborn。
- 只返回计数。一个文件同时暂存并再次修改时，staged、unstaged 各加一；不是互斥分桶。
- 冲突代码 `DD/AU/UD/UA/DU/AA/UU` 只计入 conflicted，不再重复计入 staged/unstaged。
- `??` 计入 untracked，使用 `--untracked-files=all`，目录内每个未忽略文件各计一。
- ignored 文件不计数；rename/copy 的原路径是协议续项，不是第二个文件。
- 子模块按 Git porcelain v1 的文件状态计数，不展开子模块文件树。
- 多条 Git 只读查询之间可能有并发编辑，因此快照是一次有界观察，不承诺文件系统原子快照或代码作者归属。

这些类型暂不跨 Tauri IPC，不添加 serde/TS decoder；将来跨 IPC 时必须添加 JSON 契约和 fixture 测试。

## 命令与解析边界

- 通过 `Command` 执行系统 Git，所有路径是独立 argv。
- 所有命令设置 `GIT_OPTIONAL_LOCKS=0`、`GIT_TERMINAL_PROMPT=0` 和稳定英文诊断 locale。
- 禁用 `core.fsmonitor`，避免 status 触发用户配置的外部 fsmonitor 命令；不用写操作、网络、外部 diff、hooks 或仓库清理命令。
- 清除继承的会覆盖 `-C` 目标的仓库环境变量（例如 GIT_DIR、GIT_WORK_TREE、GIT_INDEX_FILE、GIT_COMMON_DIR）。测试中不修改全局进程环境。
- 读取绝对 root/git_dir/common_dir，验证命令输出形状，不能把错误字符串当路径。
- status 使用 `--porcelain=v1 -z --untracked-files=all --ignore-submodules=none`。
- NUL 协议按字节解析，文件名不解码也不保存；支持空格、tab、换行、非 UTF-8 路径字节和 rename 双路径。
- 禁止仅按行拆分 status；遇到截断记录、未知状态码或缺失 rename 续项返回 `MalformedOutput`。
- 每条命令最多 3 秒，stdout 和 stderr 各最多保留 1 MiB。超时或任一流超过上限，终止并回收子进程并返回对应错误，不返回不完整统计。
- stdout/stderr 同时排空，避免管道满导致死锁。错误文本仅包含阶段和错误类别，不带原始 stderr、文件名、diff 或命令输出。
- 执行器只服务当前 Git 采集，不扩成全仓库通用进程框架。默认限制可通过内部测试配置缩短，以便确定性验证超时与输出限制。

## 验收

遵守 TDD：先写测试并运行得到真实失败，再实现最小功能。

1. 解析 fixture：干净、MM、staged add/delete、rename/copy、conflicts、untracked、ignored 不出现；含换行/tab/非 UTF-8 的文件名；损坏与截断输出被拒绝。
2. 临时真实仓库：空仓库、首次 commit、干净/脏、部分暂存、rename、未跟踪目录、detached HEAD、linked worktree、非仓库、裸仓库及不存在路径。
3. 采集前后比较 HEAD、index 字节和工作区文件内容，证明采集没有改写它们；fsmonitor 哨兵未执行。
4. 执行器测试：命令不可执行、失败退出、超时终止回收、stdout/stderr 并发大输出、上限错误、错误不泄漏内容。不能用实际 Git 大仓库或偶然的网络阻塞模拟超时。
5. 在本仓库运行一次生产 `capture` 的只读 smoke，结果只验证结构和 HEAD，不打印工作区文件名。
6. 如调整 Project Registry 共享路径，现有项目登记测试继续通过。
7. 全部仓库检查通过：format、rustfmt、lint、typecheck、前端测试、Rust 测试、Python 测试、build、clippy。

测试仅在唯一临时目录内执行 git init/add/commit/worktree 等准备操作；不在用户仓库写入状态。
保留已有未跟踪 handoff；不改 Session、SQLite、现有 migration、Harness 能力矩阵或版本号。
