//! `ConfigRepository` — 统一的配置接缝（R2）。
//!
//! 对应 deepseek-harness 的 `storage-domain` 纪律③：配置真相收敛到单一
//! 抽象，消费者只 `resolve(alias)` / `list_models`，不再各自 `config.toml`
//! 加载或直写后端。`pending_model` 这类双份态因此失去存在理由——它只是
//! UI 运行态里一个未 apply 的 alias，解析交给 `resolve_model`。
//!
//! 设计取舍（相对初版计划）：初版写成 `set(domain, key, json)`，但
//! `VibeConfig` 是强结构而非自由 KV，泛 JSON 写会丢失类型安全且易写错。
//! 修订版改用语义化写方法（`set_models` / `set_active_model`），并用
//! `ConfigDomain` 枚举显式标注每个写操作落在哪个配置域——既保留
//! "域划分" 思想又更 Rust、更安全。

use crate::core::config::VibeConfig;
use crate::core::{ArrowError, Result};
use std::path::PathBuf;
use std::sync::Mutex;
use tokio::sync::broadcast;

/// 配置域划分。对应 harness 把不同关注点拆成独立 `Domain` 的思路，
/// 写操作必须声明落在哪个域，避免"一个大杂烩 config"式的隐式耦合。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigDomain {
    /// 模型注册表（`[[models]]`）。
    Model,
    /// Agent 运行时配置（`active_model` / `default_agent`）。
    Agent,
}

/// 配置变更通知，对应 harness 的 `DomainChanged`。
///
/// 消费者订阅 `watch()`，在一次写操作后被广播，据此重读/刷新 UI，
/// 而不是在多处各持一份缓存再手动同步。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigChange {
    /// 变更的域（一次写通常只动一个域）。
    pub domain: ConfigDomain,
}

/// 模型摘要——列表/下拉用的轻量投影，不含密钥等敏感字段。
///
/// 对应 harness 的 `ModelSummary`：UI 派生下拉时只拿 name/model_id/provider，
/// 不碰完整 `ModelConfig`（后者含 api_key、endpoint）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelSummary {
    pub name: String,
    pub model_id: String,
    pub provider: String,
}

/// Agent 运行时配置视图——从 `VibeConfig` 投影出的精简结构。
///
/// R2 不引入庞大的新 `AgentConfig` 类型；只暴露 host/CLI 真正需要的
/// 两个字段，避免把 `VibeConfig` 整体透传（那会重新制造"消费者持有后端结构"）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentConfig {
    pub default_agent: String,
    pub default_model: Option<String>,
}

/// 配置仓库接缝。所有配置读写都经此 trait。
pub trait ConfigRepository: Send + Sync {
    /// 按 alias 解析出完整 `ModelConfig`。找不到即报错（不静默回退），
    /// 对应 harness schema 校验——错误的 alias 是调用方的 bug，不应吞掉。
    fn resolve_model(&self, alias: &str) -> Result<crate::core::config::ModelConfig>;

    /// 列出全部可用模型（轻量摘要，供 UI 派生下拉）。
    fn list_models(&self) -> Result<Vec<ModelSummary>>;

    /// 当前生效的 Agent 配置视图。
    fn current_agent_config(&self) -> Result<AgentConfig>;

    /// 写入 `Model` 域：替换整个模型注册表并持久化，随后广播变更。
    fn set_models(&self, models: Vec<crate::core::config::ModelConfig>) -> Result<()>;

    /// 写入 `Agent` 域：更新激活模型并持久化，随后广播变更。
    fn set_active_model(&self, alias: Option<&str>) -> Result<()>;

    /// 订阅配置变更（对应 harness `DomainChanged`）。
    fn watch(&self) -> broadcast::Receiver<ConfigChange>;
}

/// 本地 FS 实现：以 `VibeConfig` 为单一真相源。
///
/// 内部用 `Mutex<VibeConfig>` 持有解析后的配置；写操作经 `save_split`
/// 落盘（模型可独立到 `models.toml`），不暴露任何"直接改后端"的旁路。
pub struct LocalConfigRepository {
    inner: Mutex<VibeConfig>,
    config_path: PathBuf,
    models_path: Option<PathBuf>,
    change_tx: broadcast::Sender<ConfigChange>,
}

impl LocalConfigRepository {
    /// 解析用户/项目配置并按习惯路径构造仓库（对应旧 `VibeConfig::load_resolved`）。
    pub fn load() -> Result<Self> {
        let config = VibeConfig::load_resolved()?;
        let config_path = VibeConfig::user_config_path()
            .ok_or_else(|| ArrowError::Config("cannot determine user config path".to_string()))?;
        let models_path = config
            .models_file
            .as_ref()
            .map(|f| resolve_models_path(&config_path, f));
        Ok(Self::new(config, config_path, models_path))
    }

    /// 显式构造（host 已有解析好的 config 与路径时使用）。
    pub fn new(config: VibeConfig, config_path: PathBuf, models_path: Option<PathBuf>) -> Self {
        let (change_tx, _) = broadcast::channel(16);
        Self {
            inner: Mutex::new(config),
            config_path,
            models_path,
            change_tx,
        }
    }

    /// 只读快照：供 `build_session` 这类一次性初始化取 `bypass_tool_permissions` /
    /// `default_agent` 等参数。返回的是克隆快照，**任何写入都必须走 trait 方法**，
    /// 此处不暴露可变后端访问（消费者永不直接改后端）。
    #[allow(dead_code)]
    pub fn snapshot(&self) -> VibeConfig {
        self.inner.lock().unwrap().clone()
    }

    /// 主配置文件路径（字符串），供 UI/诊断展示。取代原 host 直接持有 `config_path`。
    pub fn config_path_display(&self) -> String {
        self.config_path.display().to_string()
    }

    /// 独立模型文件路径（字符串，若配置），供 UI/诊断展示。
    pub fn models_path_display(&self) -> Option<String> {
        self.models_path.as_ref().map(|p| p.display().to_string())
    }
}

fn resolve_models_path(config_path: &PathBuf, models_file: &str) -> PathBuf {
    let p = PathBuf::from(models_file);
    if p.is_absolute() {
        p
    } else {
        config_path
            .parent()
            .map(|d| d.join(&p))
            .unwrap_or_else(|| config_path.join(&p))
    }
}

impl ConfigRepository for LocalConfigRepository {
    fn resolve_model(&self, alias: &str) -> Result<crate::core::config::ModelConfig> {
        let cfg = self.inner.lock().unwrap();
        cfg.models
            .iter()
            .find(|m| m.name == alias)
            .cloned()
            .ok_or_else(|| ArrowError::Config(format!("unknown model alias: {}", alias)))
    }

    fn list_models(&self) -> Result<Vec<ModelSummary>> {
        let cfg = self.inner.lock().unwrap();
        Ok(cfg
            .models
            .iter()
            .map(|m| ModelSummary {
                name: m.name.clone(),
                model_id: m.model_id.clone(),
                provider: m.provider.clone(),
            })
            .collect())
    }

    fn current_agent_config(&self) -> Result<AgentConfig> {
        let cfg = self.inner.lock().unwrap();
        Ok(AgentConfig {
            default_agent: cfg.default_agent.clone(),
            default_model: cfg.active_model.clone(),
        })
    }

    fn set_models(&self, models: Vec<crate::core::config::ModelConfig>) -> Result<()> {
        {
            let mut cfg = self.inner.lock().unwrap();
            cfg.models = models;
            cfg.save_split(&self.config_path, self.models_path.as_ref())?;
        }
        let _ = self
            .change_tx
            .send(ConfigChange {
                domain: ConfigDomain::Model,
            });
        Ok(())
    }

    fn set_active_model(&self, alias: Option<&str>) -> Result<()> {
        {
            let mut cfg = self.inner.lock().unwrap();
            cfg.active_model = alias.map(|s| s.to_string());
            cfg.save_split(&self.config_path, self.models_path.as_ref())?;
        }
        let _ = self
            .change_tx
            .send(ConfigChange {
                domain: ConfigDomain::Agent,
            });
        Ok(())
    }

    fn watch(&self) -> broadcast::Receiver<ConfigChange> {
        self.change_tx.subscribe()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn test_repo() -> LocalConfigRepository {
        // 用默认配置（非真实文件），验证 repo 行为与广播，不依赖用户 config。
        let cfg = VibeConfig::with_defaults();
        LocalConfigRepository::new(cfg, PathBuf::from("config.toml"), None)
    }

    #[test]
    fn list_and_resolve_active() {
        let repo = test_repo();
        let models = repo.list_models().unwrap();
        assert!(!models.is_empty(), "default config should expose models");
        let agent = repo.current_agent_config().unwrap();
        if let Some(active) = agent.default_model {
            let m = repo.resolve_model(&active).unwrap();
            assert_eq!(m.name, active);
        }
    }

    #[test]
    fn resolve_unknown_errors() {
        let repo = test_repo();
        assert!(repo.resolve_model("__no_such_model__").is_err());
    }

    #[test]
    fn set_models_broadcasts_and_updates() {
        let repo = test_repo();
        let mut rx = repo.watch();
        let alias = repo.list_models().unwrap()[0].name.clone();
        let mut m = repo.resolve_model(&alias).unwrap();
        m.model_id = "changed-id".to_string();
        repo.set_models(vec![m.clone()]).unwrap();
        let changed = repo.list_models().unwrap();
        assert_eq!(changed.len(), 1);
        assert_eq!(changed[0].model_id, "changed-id");
        // watch 应收到 Model 域变更
        let change = rx.try_recv().expect("should receive change");
        assert_eq!(change.domain, ConfigDomain::Model);
    }

    #[test]
    fn set_active_model_broadcasts_and_updates() {
        let repo = test_repo();
        let mut rx = repo.watch();
        repo.set_active_model(Some("m_test")).unwrap();
        assert_eq!(
            repo.current_agent_config().unwrap().default_model,
            Some("m_test".to_string())
        );
        let change = rx.try_recv().expect("should receive change");
        assert_eq!(change.domain, ConfigDomain::Agent);
    }
}
