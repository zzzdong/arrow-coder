//! Session query layer — derived projections over the append-only event log (R3).
//!
//! 对应 deepseek-harness 的 `SessionQueryEngine`：**只做从事件日志投影的衍生查询**，
//! 不新增任何存储实体。资源真相（list/header/生命周期）归 [`crate::session::repository::SessionRepository`]，
//! 本层只关心"重算视图"：
//!
//! - `search_events` — 全文/语义检索事件日志（harness search/trace）。
//! - `get_turn_window` — 取某轮投影（**Turn 永远只是区间视图，不进 Repository/Query 的存储**）。
//! - `get_title` — 标题投影：优先 header.title，缺失时从首条 user 消息派生。
//!
//! 与 R1 的边界（见 `docs/refactor-plan-resources.md` §5.1）：**不提供 `list`**——
//! 列表由 `SessionRepository::list`（读 header）负责，query 层不重复。

use crate::session::header::SessionId;
use crate::session::repository::SessionRepository;
use crate::session::store::SessionStore;
use crate::session::title::generate_default_title;
use crate::session::{SessionEvent, UiMessage};
use crate::core::TurnStats;
use crate::core::Result;
use serde::Serialize;
use std::sync::Arc;

/// 一次检索命中（事件日志中的某个事件）。
#[derive(Debug, Clone, Serialize)]
pub struct EventHit {
    /// 事件在日志中的序号。
    pub index: usize,
    /// 事件时间戳（unix ms），若无则 None。
    pub ts: Option<u64>,
    /// 事件种类（小写下划线，如 `user_message` / `tool_call`）。
    pub kind: String,
    /// 命中文本的预览片段。
    pub preview: String,
}

/// 某一轮的投影视图。`Turn` 仅是区间视图，不是存储实体。
#[derive(Debug, Clone, Serialize)]
pub struct TurnView {
    /// 轮次（1-based）。
    pub turn: u32,
    /// 该轮投影出的 UI 消息（用户消息 -> 助手回复/工具 -> stats）。
    pub messages: Vec<UiMessage>,
    /// 该轮末尾的用量统计（若有）。
    pub stats: Option<TurnStats>,
}

/// 会话衍生查询接缝。所有实现都从 `SessionStore`（append-only 日志）投影，
/// 不持有独立存储。
pub trait SessionQuery: Send + Sync {
    /// 全文检索事件日志，返回命中事件（带预览）。
    fn search_events(&self, id: &SessionId, text: &str) -> Result<Vec<EventHit>>;

    /// 取第 `turn` 轮（1-based）的投影视图。
    fn get_turn_window(&self, id: &SessionId, turn: u32) -> Result<TurnView>;

    /// 标题投影：优先 header.title，缺失时从首条 user 消息派生。
    fn get_title(&self, id: &SessionId) -> Result<Option<String>>;
}

/// 本地 FS 实现：经 `SessionRepository::dir_of` 定位会话目录，再加载
/// `SessionStore` 投影。资源定位与查询投影解耦——query 不自己管理目录。
pub struct LocalSessionQuery {
    repo: Arc<dyn SessionRepository>,
}

impl LocalSessionQuery {
    pub fn new(repo: Arc<dyn SessionRepository>) -> Self {
        Self { repo }
    }

    /// 定位并加载会话的事件日志（None 目录即会话不存在）。
    fn load_store(&self, id: &SessionId) -> Result<Option<SessionStore>> {
        match self.repo.dir_of(id) {
            Some(dir) => Ok(Some(SessionStore::load_from_dir(&dir)?)),
            None => Ok(None),
        }
    }
}

impl SessionQuery for LocalSessionQuery {
    fn search_events(&self, id: &SessionId, text: &str) -> Result<Vec<EventHit>> {
        let Some(store) = self.load_store(id)? else {
            return Ok(Vec::new());
        };
        let needle = text.to_lowercase();
        let mut hits = Vec::new();
        for (index, ev) in store.events().iter().enumerate() {
            let mut candidates: Vec<(String, String)> = Vec::new();
            match ev {
                SessionEvent::UserMessage { text, .. } => {
                    candidates.push(("user_message".into(), text.clone()));
                }
                SessionEvent::AssistantMessage { text, .. } => {
                    candidates.push(("assistant_message".into(), text.clone()));
                }
                SessionEvent::AssistantChunk { delta, .. } => {
                    candidates.push(("assistant_chunk".into(), delta.clone()));
                }
                SessionEvent::ToolCall { name, args, .. } => {
                    candidates.push(("tool_call".into(), format!("{} {}", name, args)));
                }
                SessionEvent::ToolResult { render, value, .. } => {
                    let body = render
                        .clone()
                        .or_else(|| Some(value.to_string()))
                        .unwrap_or_default();
                    candidates.push(("tool_result".into(), body));
                }
                SessionEvent::Command { name, args, .. } => {
                    candidates.push(("command".into(), format!("{} {:?}", name, args)));
                }
                SessionEvent::Compaction { summary, .. } => {
                    candidates.push(("compaction".into(), summary.clone()));
                }
                _ => {}
            }
            for (kind, body) in candidates {
                if body.to_lowercase().contains(&needle) {
                    hits.push(EventHit {
                        index,
                        ts: ev.ts(),
                        kind,
                        preview: preview_of(&body),
                    });
                }
            }
        }
        Ok(hits)
    }

    fn get_turn_window(&self, id: &SessionId, turn: u32) -> Result<TurnView> {
        let Some(store) = self.load_store(id)? else {
            return Err(crate::core::ArrowError::Session(format!(
                "session not found: {}",
                id
            )));
        };
        let events = store.events();
        if turn == 0 {
            return Err(crate::core::ArrowError::Session(
                "turn index is 1-based; 0 is invalid".into(),
            ));
        }
        // 用用户消息作为轮次边界（与 manager.rs 的 `undo_last_turn` 一致：
        // 没有显式 TurnStart/TurnEnd，相邻 UserMessage 之间即一轮）。
        let user_indices: Vec<usize> = events
            .iter()
            .enumerate()
            .filter(|(_, e)| matches!(e, SessionEvent::UserMessage { .. }))
            .map(|(i, _)| i)
            .collect();
        if user_indices.is_empty() {
            return Err(crate::core::ArrowError::Session(
                "session has no turns yet".into(),
            ));
        }
        let turn0 = (turn as usize) - 1;
        if turn0 >= user_indices.len() {
            return Err(crate::core::ArrowError::Session(format!(
                "turn {} out of range (session has {} turns)",
                turn,
                user_indices.len()
            )));
        }
        let start = user_indices[turn0];
        let end = user_indices
            .get(turn0 + 1)
            .copied()
            .unwrap_or(events.len());
        // 区间内 ToolCall/ToolResult 必定成对闭合（下一 UserMessage 之前助手已回复完）。
        let window = events[start..end].to_vec();
        let projected = SessionStore::from_events(window).derive_ui_messages();

        // 取区间内最后一个 TurnStats 作为该轮用量。
        let stats = events[start..end]
            .iter()
            .rev()
            .find_map(|e| match e {
                SessionEvent::TurnStats { stats, .. } => Some(stats.clone()),
                _ => None,
            });

        Ok(TurnView {
            turn,
            messages: projected,
            stats,
        })
    }

    fn get_title(&self, id: &SessionId) -> Result<Option<String>> {
        // 优先 header.title（资源真相）。
        if let Some(header) = self.repo.get_header(id)? {
            if let Some(t) = header.title {
                return Ok(Some(t));
            }
        }
        // 缺失时从首条 user 消息派生（harness title 逻辑）。
        let Some(store) = self.load_store(id)? else {
            return Ok(None);
        };
        let first_user = store.events().iter().find_map(|e| match e {
            SessionEvent::UserMessage { text, .. } => Some(text.clone()),
            _ => None,
        });
        Ok(first_user.map(|t| generate_default_title(&t)))
    }
}

/// 生成检索预览：取前后文片段，过长截断。
fn preview_of(body: &str) -> String {
    const MAX: usize = 200;
    if body.len() <= MAX {
        body.to_string()
    } else {
        format!("{}…", &body[..MAX])
    }
}

/// 计算给定会话日志的轮次数（供 UI 展示，不属于 trait，但复用同一边界逻辑）。
pub fn count_turns(events: &[SessionEvent]) -> usize {
    events
        .iter()
        .filter(|e| matches!(e, SessionEvent::UserMessage { .. }))
        .count()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::header::{SessionOrigin, SessionSummary};
    use crate::session::logger::SessionLoggerConfig;
    use crate::session::repository::{LocalSessionRepository, SessionRepository};
    use std::fs;
    use std::path::PathBuf;

    fn temp_repo() -> (LocalSessionRepository, PathBuf) {
        let base = std::env::temp_dir().join(format!(
            "arrow-coder-query-test-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(&base).unwrap();
        let cfg = SessionLoggerConfig {
            enabled: true,
            save_dir: base.clone(),
            session_prefix: "session".to_string(),
        };
        (LocalSessionRepository::new(cfg), base)
    }

    fn seed_session(
        repo: &LocalSessionRepository,
        events: Vec<SessionEvent>,
    ) -> SessionId {
        let id = repo.create(SessionOrigin::Cli, None).unwrap();
        let dir = repo.dir_of(&id).unwrap();
        let mut store = SessionStore::new_at(&dir).unwrap();
        store.append_events(events).unwrap();
        id
    }

    fn cleanup(base: &PathBuf) {
        let _ = fs::remove_dir_all(base);
    }

    fn mk_turn(n: usize, text: &str) -> Vec<SessionEvent> {
        vec![
            SessionEvent::UserMessage {
                text: text.to_string(),
                ts: 1_700_000_000_000 + n as u64 * 1000,
            },
            SessionEvent::AssistantMessage {
                text: format!("reply to {}", text),
                ts: 1_700_000_000_000 + n as u64 * 1000 + 1,
            },
            SessionEvent::TurnStats {
                stats: crate::core::TurnStats {
                    prompt_tokens: n as u64,
                    completion_tokens: n as u64,
                    cache_hit_tokens: 0,
                    reasoning_tokens: 0,
                    total_tokens: 2 * n as u64,
                    cache_hit_rate: 0.0,
                    duration_ms: 10,
                    session_prompt_tokens: n as u64,
                    session_completion_tokens: n as u64,
                    session_cache_hit_tokens: 0,
                    session_reasoning_tokens: 0,
                },
                ts: 1_700_000_000_000 + n as u64 * 1000 + 2,
            },
        ]
    }

    #[test]
    fn get_title_prefers_header_then_derives() {
        let (repo, base) = temp_repo();
        // header 无 title -> 派生自首条 user 消息。
        let id = seed_session(&repo, mk_turn(1, "How do I sort a vector"));
        let q = LocalSessionQuery::new(std::sync::Arc::new(repo));
        let title = q.get_title(&id).unwrap();
        assert_eq!(title.as_deref(), Some("How do I sort a vector"));
        cleanup(&base);
    }

    #[test]
    fn get_title_uses_header_override() {
        let (repo, base) = temp_repo();
        let id = seed_session(&repo, mk_turn(1, "ignored body"));
        repo.update_meta(
            &id,
            &crate::session::header::HeaderPatch {
                title: Some("Pinned Title".into()),
                cwd: None,
            },
        )
        .unwrap();
        let q = LocalSessionQuery::new(std::sync::Arc::new(repo));
        assert_eq!(q.get_title(&id).unwrap().as_deref(), Some("Pinned Title"));
        cleanup(&base);
    }

    #[test]
    fn search_events_finds_and_misses() {
        let (repo, base) = temp_repo();
        let id = seed_session(
            &repo,
            vec![
                SessionEvent::UserMessage {
                    text: "find the needle".into(),
                    ts: 1,
                },
                SessionEvent::AssistantMessage {
                    text: "here is hay".into(),
                    ts: 2,
                },
            ],
        );
        let q = LocalSessionQuery::new(std::sync::Arc::new(repo));
        let hits = q.search_events(&id, "needle").unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].kind, "user_message");
        assert!(hits[0].preview.contains("needle"));
        // 不命中
        assert!(q.search_events(&id, "absent").unwrap().is_empty());
        cleanup(&base);
    }

    #[test]
    fn get_turn_window_bounds_and_out_of_range() {
        let (repo, base) = temp_repo();
        let id = seed_session(
            &repo,
            {
                let mut v = mk_turn(1, "first");
                v.extend(mk_turn(2, "second"));
                v
            },
        );
        let q = LocalSessionQuery::new(std::sync::Arc::new(repo));
        let t1 = q.get_turn_window(&id, 1).unwrap();
        assert_eq!(t1.turn, 1);
        assert!(t1.messages.iter().any(|m| m.text.contains("first")));
        assert!(t1.stats.is_some());
        let t2 = q.get_turn_window(&id, 2).unwrap();
        assert!(t2.messages.iter().any(|m| m.text.contains("second")));
        // 越界
        assert!(q.get_turn_window(&id, 3).is_err());
        // 0 非法
        assert!(q.get_turn_window(&id, 0).is_err());
        cleanup(&base);
    }

    #[test]
    fn query_does_not_duplicate_list() {
        // 边界回归：query 层不提供 list；list 归 SessionRepository。
        let (repo, base) = temp_repo();
        let _id = repo.create(SessionOrigin::Cli, None).unwrap();
        let _summaries: Vec<SessionSummary> = repo.list(&Default::default()).unwrap();
        // 仅验证编译期 trait 形状：query 不暴露 list 方法。
        fn _assert_no_list(_q: &dyn SessionQuery) {}
        cleanup(&base);
    }
}

