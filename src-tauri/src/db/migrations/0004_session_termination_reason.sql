-- 0004 —— Session 增加 termination_reason：把「进程怎么结束」与「退出码」分开。
--
-- 问题：exit_code 非零不等于同一种「失败」。以下都可能是非零退出，含义完全不同：
--
--     用户主动 Kill            → 是我们让他停的，不是 Agent 工作失败
--     Codex 自身 Ctrl+C        → 用户中断
--     CLI 参数错误             → 启动即失败
--     Agent 工作失败           → 真的业务失败
--     Harness Hub 自己管理进程失败 → 是宿主的问题，不是 Harness 的问题
--
-- 以后再叠加 Analytics（Task 5+ usage / 统计）时，没有这个字段就只能把所有非零退出
-- 都算成同一种「失败」，结论会直接错。
--
-- 语义：
--     termination_reason = 为什么结束
--     exit_code          = 进程自己的退出码（可能为 NULL）
--
-- 允许 NULL：created / running 阶段没有终止原因；迁移前的存量终态行也无法可靠反推
-- （例如旧的 'failed' 分不清是启动失败还是运行失败），因此留 NULL 表示「原因未记录」，
-- 而不是硬塞一个看似正确的值。

ALTER TABLE sessions ADD COLUMN termination_reason TEXT
    CHECK (
        termination_reason IS NULL
        OR termination_reason IN (
            'natural_exit',
            'user_killed',
            'launch_failed',
            'runtime_error',
            'host_shutdown',
            'lost'
        )
    );

-- 能可靠反推的存量行补上原因：正常结束 / 拿不到终态。
UPDATE sessions SET termination_reason = 'natural_exit' WHERE status = 'exited';
UPDATE sessions SET termination_reason = 'lost' WHERE status = 'unknown';

CREATE INDEX idx_sessions_termination_reason ON sessions (termination_reason);
