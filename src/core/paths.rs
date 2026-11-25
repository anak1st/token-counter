//! 跨平台用户目录解析（兼容 macOS / Linux 与 Windows）。
//!
//! Windows 默认不设置 `HOME`，改用 `USERPROFILE`、`LOCALAPPDATA`、`APPDATA`；
//! 此模块集中屏蔽环境变量差异，避免各处重复判断。

use std::env;
use std::path::PathBuf;

/// 读取非空环境变量。
fn non_empty_env_path(key: &str) -> Option<PathBuf> {
    let val = env::var_os(key)?;
    if val.is_empty() {
        None
    } else {
        Some(PathBuf::from(val))
    }
}

/// 按顺序向候选列表追加目录并去重。
fn push_unique(dirs: &mut Vec<PathBuf>, candidate: Option<PathBuf>) {
    if let Some(path) = candidate {
        if !dirs.contains(&path) {
            dirs.push(path);
        }
    }
}

/// 解析当前用户主目录（依次检查 `HOME`、`USERPROFILE`）。
pub fn user_home_dir() -> Option<PathBuf> {
    non_empty_env_path("HOME").or_else(|| non_empty_env_path("USERPROFILE"))
}

/// 返回按优先级排列的用户配置根目录列表：
/// 1. `$XDG_CONFIG_HOME`
/// 2. `~/.config`（兼容 `HOME` 与 `USERPROFILE`）
/// 3. `%APPDATA%`（Windows 标准配置目录）
pub fn user_config_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    push_unique(&mut dirs, non_empty_env_path("XDG_CONFIG_HOME"));
    push_unique(&mut dirs, user_home_dir().map(|h| h.join(".config")));
    push_unique(&mut dirs, non_empty_env_path("APPDATA"));
    dirs
}

/// 返回按优先级排列的用户数据根目录列表：
/// 1. `$XDG_DATA_HOME`
/// 2. `~/.local/share`（兼容 `HOME` 与 `USERPROFILE`）
/// 3. `%LOCALAPPDATA%`（Windows 本地数据目录）
/// 4. `%APPDATA%`（Windows 漫游数据目录兜底）
pub fn user_data_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    push_unique(&mut dirs, non_empty_env_path("XDG_DATA_HOME"));
    push_unique(
        &mut dirs,
        user_home_dir().map(|h| h.join(".local").join("share")),
    );
    push_unique(&mut dirs, non_empty_env_path("LOCALAPPDATA"));
    push_unique(&mut dirs, non_empty_env_path("APPDATA"));
    dirs
}

/// 返回候选目录列表中首个已存在的文件路径，若均不存在则返回首个候选路径。
pub fn first_existing_or_default(candidates: Vec<PathBuf>) -> Option<PathBuf> {
    if let Some(found) = candidates.iter().find(|p| p.is_file()) {
        return Some(found.clone());
    }
    candidates.into_iter().next()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn push_unique_deduplicates_preserving_order() {
        let mut dirs = Vec::new();
        push_unique(&mut dirs, Some(PathBuf::from("/tmp/a")));
        push_unique(&mut dirs, Some(PathBuf::from("/tmp/b")));
        push_unique(&mut dirs, Some(PathBuf::from("/tmp/a")));
        push_unique(&mut dirs, None);
        assert_eq!(dirs, vec![PathBuf::from("/tmp/a"), PathBuf::from("/tmp/b")]);
    }
}
