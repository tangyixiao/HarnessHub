//! B6 真机只读 smoke：对本机真实 `~/.claude/projects/**/*.jsonl` 做只读回放解析。
//!
//! 门控：本机没有 `~/.claude/projects`（或设 `HH_SKIP_REAL_CLAUDE_REPLAY=1`）时
//! 明确跳过——真机证据只该在有真机的机器上产生。
//! 只读纪律：不复制内容、不落库、不写任何文件；断言只看结构与计数。

use std::path::PathBuf;

use harness_hub_lib::replay::claude::{list_sessions, read_session_by_id};

fn claude_root() -> Option<PathBuf> {
    let home = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME"))?;
    let root = PathBuf::from(home).join(".claude");
    root.join("projects").is_dir().then_some(root)
}

#[test]
fn real_claude_transcripts_list_and_read_read_only() {
    let Some(root) = claude_root() else {
        eprintln!("SKIP：本机没有 ~/.claude/projects，真机回放证据待有数据的机器补");
        return;
    };
    if std::env::var_os("HH_SKIP_REAL_CLAUDE_REPLAY").is_some() {
        eprintln!("SKIP：HH_SKIP_REAL_CLAUDE_REPLAY=1");
        return;
    }

    let roots = [root];
    let summaries = list_sessions(&roots);
    assert!(!summaries.is_empty(), "本机存在转录但发现为空——发现层坏了");
    eprintln!(
        "发现 {} 条可回放会话，最新：{:?}",
        summaries.len(),
        summaries.first().map(|s| &s.session_id)
    );

    // 摘要的健康断言：id 非空、按 mtime 倒序、体积非零。
    assert!(summaries.iter().all(|s| !s.session_id.is_empty()));
    assert!(summaries.windows(2).all(|pair| pair[0].modified_ms >= pair[1].modified_ms));
    assert!(summaries.iter().all(|s| s.size_bytes > 0));

    // 真机事实（2026-10-25）：存在大量「只有 permission-mode 一行」的空转录
    // （会话启动后没说话就退出——本机 E2E 测试的常态产物）。空转录是合法数据：
    // 解析不得崩、消息为空。扫到第一条非空的做深检（空存根都极小，代价可忽略）。
    let mut empty_seen = 0usize;
    let mut non_empty: Option<harness_hub_lib::replay::ReplaySession> = None;
    for summary in &summaries {
        let session = read_session_by_id(&roots, &summary.session_id)
            .expect("真机读取不得报错")
            .unwrap_or_else(|| panic!("刚发现的 session 读不回：{}", summary.session_id));
        if session.messages.is_empty() {
            empty_seen += 1;
            continue;
        }
        non_empty = Some(session);
        break;
    }

    let session = non_empty.expect("前 20 条转录全是空的——本机数据不足以做深检");
    eprintln!(
        "回放 {}：{} 条消息，skipped={:?}，malformed={}（另见 {empty_seen} 条空转录）",
        session.session_id,
        session.messages.len(),
        session.skipped_kind_counts,
        session.malformed_lines
    );
    assert!(!session.messages.is_empty());
    assert!(session.messages.iter().enumerate().all(|(index, m)| m.seq == index as u64));

    // 路径白名单：随机 id 必须 None。
    assert!(read_session_by_id(&roots, "no-such-session-id").unwrap().is_none());
}
