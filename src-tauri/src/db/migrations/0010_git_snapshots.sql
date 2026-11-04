-- 0010_git_snapshots —— Git Session Snapshot（ROADMAP 阶段 A1）。
--
-- 语义：会话生命周期两个时点（start = 进程启动成功后 / end = 终态写入成功后）
-- 的有界只读仓库快照。它只表达「会话时间窗口内仓库的状态」，不能证明代码由
-- AI 编写（docs/CONTEXT.md 核心约束 7 / 规划 R5）。
--
-- 快照是 best-effort 观测：cwd 缺失 / 非 Git 仓库 / git 命令失败时允许没有行，
-- 因此「没有快照」不代表「没有变化」。
-- 同一 (session, phase) 至多一行：终态路径可能重复触发补拍，靠主键幂等去重。
CREATE TABLE git_snapshots (
    hub_session_id   TEXT NOT NULL REFERENCES sessions (hub_session_id) ON DELETE CASCADE,
    phase            TEXT NOT NULL CHECK (phase IN ('start', 'end')),
    captured_at      TEXT NOT NULL,
    worktree_root    TEXT NOT NULL,
    -- 合法的 unborn HEAD（空仓库）为 NULL。
    head             TEXT,
    staged_files     INTEGER NOT NULL CHECK (staged_files >= 0),
    unstaged_files   INTEGER NOT NULL CHECK (unstaged_files >= 0),
    untracked_files  INTEGER NOT NULL CHECK (untracked_files >= 0),
    conflicted_files INTEGER NOT NULL CHECK (conflicted_files >= 0),
    PRIMARY KEY (hub_session_id, phase)
);
