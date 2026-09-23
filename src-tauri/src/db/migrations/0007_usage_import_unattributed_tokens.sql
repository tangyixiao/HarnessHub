-- 0007: 记录「上游汇总里没有落到任何模型明细上」的 token 数。
--
-- 依据（ADR-0011 决策十一，实测而非推测）：
--   222 行 session 里 221 行的 `totalTokens` 恰好等于四类 token 之和，
--   但有 1 行（opencode，`ses_f40ea5cdbffeienAXgo4uTUBsL`）比明细多 910。
--   这部分 token 无法归因到任何模型，因此不能写进任何一条 `usage_events`；
--   如果不记下来，`Σ(事件) vs totals` 的差额就成了「未知」，而对账不允许有未知差额。
--
-- ADD COLUMN 带 NOT NULL DEFAULT 会把老行补成 0（而不是 NULL），
-- 这是有意的：老导入行没有这个事实，记 0 表示「没有观察到差额」。

ALTER TABLE usage_imports
    ADD COLUMN unattributed_tokens INTEGER NOT NULL DEFAULT 0
    CHECK (unattributed_tokens >= 0);
