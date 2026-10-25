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
    assert!(summaries
        .windows(2)
        .all(|pair| pair[0].modified_ms >= pair[1].modified_ms));
    assert!(summaries.iter().all(|s| s.size_bytes > 0));

    // 真机事实（2026-10-25）：存在大量「只有 permission-mode 一行」的空转录
    // （会话启动后没说话就退出——本机 E2E 测试的常态产物）。空转录是合法数据：
    // 解析不得崩、消息为空。扫到第一条非空的做深检（空存根都极小，代价可忽略）。
    let mut empty_seen = 0usize;
    let mut non_empty: Option<harness_hub_lib::replay::ReplaySession> = None;
    let mut with_tools: Option<harness_hub_lib::replay::ReplaySession> = None;
    for summary in &summaries {
        let session = read_session_by_id(&roots, &summary.session_id)
            .expect("真机读取不得报错")
            .unwrap_or_else(|| panic!("刚发现的 session 读不回：{}", summary.session_id));
        if session.messages.is_empty() {
            empty_seen += 1;
            continue;
        }
        if with_tools.is_none() && session.messages.iter().any(|m| !m.tool_calls.is_empty()) {
            with_tools = Some(session.clone());
        }
        if non_empty.is_none() {
            non_empty = Some(session);
        }
        if with_tools.is_some() {
            break;
        }
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
    assert!(session
        .messages
        .iter()
        .enumerate()
        .all(|(index, m)| m.seq == index as u64));

    // B8 结构自洽断言对非空会话与「带工具的会话」都跑。
    let sessions_to_check = with_tools.iter().chain(std::iter::once(&session));
    for session in sessions_to_check {
        let by_seq: std::collections::BTreeMap<u64, &harness_hub_lib::replay::ReplayMessage> =
            session.messages.iter().map(|m| (m.seq, m)).collect();
        for thread in &session.subagent_threads {
            assert!(!thread.message_seqs.is_empty());
            for seq in &thread.message_seqs {
                let message = by_seq
                    .get(seq)
                    .unwrap_or_else(|| panic!("线程引用了不存在的 seq {seq}"));
                assert!(message.sidechain, "线程 seq {seq} 必须是 sidechain 消息");
            }
        }
        for message in &session.messages {
            for call in &message.tool_calls {
                assert!(!call.name.is_empty());
                assert!(
                    call.input_json.len() <= 2 * 1024 + 3,
                    "截断上限（2048 字节）+ 省略号，实际 {}",
                    call.input_json.len()
                );
            }
        }
    }
    if let Some(with_tools) = &with_tools {
        let tool_count: usize = with_tools.messages.iter().map(|m| m.tool_calls.len()).sum();
        eprintln!(
            "带工具会话 {}：{} 条消息共 {tool_count} 个 tool_call，{} 条子代理线程",
            with_tools.session_id,
            with_tools.messages.len(),
            with_tools.subagent_threads.len()
        );
        assert!(tool_count > 0);
    } else {
        eprintln!("本机扫描范围内没有带 tool_call 的转录（工具深检由 fixture 覆盖）");
    }

    // 路径白名单：随机 id 必须 None。
    assert!(read_session_by_id(&roots, "no-such-session-id")
        .unwrap()
        .is_none());
}
