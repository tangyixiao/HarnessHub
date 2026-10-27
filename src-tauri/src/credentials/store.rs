//! Secret Store 抽象 + FileSecretStore 兜底（ROADMAP E16）。
//!
//! **值只进不出**：本模块所有公开接口不提供读取明文的路径
//! （`exists` 只回答「在不在」）。真实 OS keychain（Windows Credential Manager
//! 经 `windows-sys` CredWrite）是后续真机验收项；`FileSecretStore` 是
//! **开发 / CI 兜底**，值以 0600 权限写 `~/.harness-hub/secrets/<ref>`，
//! 生产分发应换 OS store（TODO 标注在模块级）。

use std::path::PathBuf;

use crate::error::{Error, Result};

/// Secret 存储抽象。值**只进不出**。
pub(crate) trait SecretStore {
    fn set(&self, ref_id: &str, value: &str) -> Result<()>;
    fn delete(&self, ref_id: &str) -> Result<()>;
    fn exists(&self, ref_id: &str) -> Result<bool>;
}

fn secrets_dir() -> PathBuf {
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    home.join(".harness-hub").join("secrets")
}

/// 文件兜底存储。ref_id 即文件名（UUID，无路径注入面）。
pub(crate) struct FileSecretStore;

impl SecretStore for FileSecretStore {
    fn set(&self, ref_id: &str, value: &str) -> Result<()> {
        if ref_id.contains('/') || ref_id.contains('\\') || ref_id.contains("..") {
            return Err(Error::InvalidInput("非法的凭据引用 id".to_string()));
        }
        let path = secrets_dir().join(ref_id);
        std::fs::create_dir_all(secrets_dir())?;
        std::fs::write(&path, value)?;
        // 0600：仅当前用户可读（POSIX；Windows 上 NTFS 默认 ACL 已限当前用户）。
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
        }
        Ok(())
    }

    fn delete(&self, ref_id: &str) -> Result<()> {
        let path = secrets_dir().join(ref_id);
        match std::fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error.into()),
        }
    }

    fn exists(&self, ref_id: &str) -> Result<bool> {
        Ok(secrets_dir().join(ref_id).is_file())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_exists_delete_round_trip() {
        let store = FileSecretStore;
        let id = uuid::Uuid::new_v4().to_string();
        assert!(!store.exists(&id).unwrap());
        store.set(&id, "secret-value").unwrap();
        assert!(store.exists(&id).unwrap());
        store.delete(&id).unwrap();
        assert!(!store.exists(&id).unwrap());
    }

    /// 路径注入拒绝（ref_id 不可携带分隔符）。
    #[test]
    fn rejects_path_traversal_ref_ids() {
        let store = FileSecretStore;
        assert!(store.set("../../etc/passwd", "x").is_err());
        assert!(store.set("a/b", "x").is_err());
        assert!(store.set("a\\b", "x").is_err());
        assert!(store.set("..\\x", "x").is_err());
    }

    /// 删除不存在的 ref 是幂等成功（remove 流程先 store 后行）。
    #[test]
    fn delete_missing_is_idempotent() {
        let store = FileSecretStore;
        store
            .delete(&uuid::Uuid::new_v4().to_string())
            .expect("删除不存在的 ref 必须幂等");
    }
}
