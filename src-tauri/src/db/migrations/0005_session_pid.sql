-- 0005 —— Session 记录进程 PID。
--
-- 用途（不把 PID 当永久身份，但诊断价值很高）：
--     ghost running 排查
--     kill 目标定位
--     host crash 后的一致性核对
--     runtime reconciliation（重启后判断进程是否还活着）
--
-- 允许 NULL：created / 导入的会话 / spawn 未能拿到 pid 时都没有值。
-- 未来 Remote Runtime 会升级成更稳定的 process identity，PID 只是本机诊断线索。

ALTER TABLE sessions ADD COLUMN pid INTEGER;
