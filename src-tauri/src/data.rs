//! 数据层：data.json 的加载 / 原子写 / 滚动备份
//!
//! 目录规则（见后端方案 §6）：程序所在文件夹下的 `data\`——
//! 开发期 exe 在 src-tauri\target\debug\，data 就落在它旁边；
//! 打包安装后 exe 在安装目录里，data 也跟着走：拷走文件夹 = 带走一切。
//!
//! 写入策略：先写 data.json.tmp 再 rename 覆盖（原子替换，断电不坏库）；
//! 每次启动时滚一份备份进 backups\，只保留最近 10 份。

use std::{
    fs,
    path::{Path, PathBuf},
    sync::Mutex,
};

use serde::{Deserialize, Serialize};

use crate::theme::ThemeTokens;

pub const SCHEMA_VERSION: u32 = 1;

// ———————— 数据结构（字段名与后端方案 §6 的 JSON 一一对应）————————

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct RemindLogEntry {
    pub planned_at: String,
    pub fired_at: String,
    pub status: String,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Event {
    pub id: String,
    pub title: String,
    pub start_date: String,
    pub start_time: Option<String>, // null = 全天
    pub end_date: String,
    pub completed: bool,
    pub completed_at: Option<String>,
    pub remind_enabled: bool,
    pub remind_at_override: Option<String>, // null = 走默认规则（最后一天前一天 07:30）
    pub remind_log: Vec<RemindLogEntry>,
    pub source_text: Option<String>,
    pub source_fragment: Option<String>,
    // M6.2 重复事件
    pub recurrence: Option<String>,          // "daily" | "weekly" | "monthly" | "yearly" | null
    pub recurrence_days_of_week: Option<Vec<u8>>, // weekly: 0=Sun..6=Sat
    pub recurrence_until: Option<String>,    // YYYY-MM-DD 重复截止日
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct AiProvider {
    pub preset: String,
    pub model: String,
    // 注意：API 密钥不落 data.json（安全方案 §11.4，M3 接 Windows 凭据管理器）
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct WidgetSettings {
    pub enabled: bool,
    pub position: String,
    pub always_on_top: bool,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct SourceSettings {
    pub paste: bool,
    pub ocr: bool,
    pub wechat: bool, // 预留：恒为 false（方案 §10.4）
    pub qq: bool,     // 预留
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    pub default_remind_time: String, // "07:30"
    pub catch_up_on_start: bool,
    pub ai_parse_enabled: bool,
    pub ai_provider: AiProvider,
    pub auto_launch: bool,
    pub widget: WidgetSettings,
    pub sources: SourceSettings,
    pub current_theme: Option<ThemeTokens>, // M4 主题持久化
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            default_remind_time: "07:30".into(),
            catch_up_on_start: true,
            ai_parse_enabled: false,
            ai_provider: AiProvider {
                preset: "deepseek".into(),
                model: String::new(),
            },
            auto_launch: true,
            widget: WidgetSettings {
                enabled: true,
                position: "右上角".into(),
                always_on_top: true,
            },
            sources: SourceSettings {
                paste: true,
                ocr: true,
                wechat: false,
                qq: false,
            },
            current_theme: None,
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct WaterDay {
    pub date: String, // YYYY-MM-DD
    pub water: u32,   // 该日结算滴数（0 = 未浇水，仅占位）
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct Garden {
    pub water_total: u32,
    pub streak: u32,
    pub max_streak: u32,
    pub water_log: Vec<WaterDay>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Theme {
    pub id: String,
    pub name: String,
    pub photo: String,
    pub active: bool,
    pub tokens: serde_json::Value, // M4 取色管线填充
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct AppData {
    pub version: u32,
    pub settings: Settings,
    pub events: Vec<Event>,
    pub themes: Vec<Theme>,
    pub garden: Garden,
}

impl Default for AppData {
    fn default() -> Self {
        Self {
            version: SCHEMA_VERSION,
            settings: Settings::default(),
            events: Vec::new(),
            themes: Vec::new(),
            garden: Garden::default(),
        }
    }
}

// ———————— 存储 ————————

pub struct DataStore {
    pub dir: PathBuf,
    state: Mutex<AppData>,
}

impl DataStore {
    /// 启动初始化：确保目录存在 → 载入（损坏则留证重建）→ 备份 → 落盘
    pub fn init() -> Result<Self, String> {
        let dir = data_dir()?;
        fs::create_dir_all(dir.join("themes")).map_err(|e| format!("创建 themes 目录失败: {e}"))?;
        fs::create_dir_all(dir.join("backups")).map_err(|e| format!("创建 backups 目录失败: {e}"))?;
        let file = dir.join("data.json");

        let state = match fs::read_to_string(&file) {
            Ok(text) => match serde_json::from_str::<AppData>(&text) {
                Ok(data) => data,
                Err(e) => {
                    // 解析失败：把损坏现场挪进 backups 留证，再用默认结构重建
                    let name = format!("data-corrupt-{}.json", stamp());
                    let _ = fs::rename(&file, dir.join("backups").join(name));
                    eprintln!("[data] data.json 解析失败（{e}），已备份现场并重建");
                    AppData::default()
                }
            },
            Err(_) => AppData::default(), // 首次运行
        };

        let store = Self {
            dir,
            state: Mutex::new(state),
        };
        store.backup_on_startup();
        let guard = store.state.lock().map_err(|e| e.to_string())?;
        store.write_to_disk(&guard)?;
        drop(guard);
        Ok(store)
    }

    /// 只读访问
    pub fn with<F, R>(&self, f: F) -> R
    where
        F: FnOnce(&AppData) -> R,
    {
        let guard = self.state.lock().expect("data lock poisoned");
        f(&guard)
    }

    /// 读写访问：闭包返回 Err 时整体不落盘
    pub fn with_mut<F, R>(&self, f: F) -> Result<R, String>
    where
        F: FnOnce(&mut AppData) -> Result<R, String>,
    {
        let mut guard = self.state.lock().map_err(|e| e.to_string())?;
        let result = f(&mut guard)?;
        self.write_to_disk(&guard)?;
        Ok(result)
    }

    /// 原子写：tmp 文件 + rename 覆盖
    fn write_to_disk(&self, data: &AppData) -> Result<(), String> {
        let file = self.dir.join("data.json");
        let tmp = self.dir.join("data.json.tmp");
        let text = serde_json::to_string_pretty(data).map_err(|e| format!("序列化失败: {e}"))?;
        fs::write(&tmp, text).map_err(|e| format!("写临时文件失败: {e}"))?;
        fs::rename(&tmp, &file).map_err(|e| format!("原子替换失败: {e}"))?;
        Ok(())
    }

    /// 启动时备份当前 data.json 并清理旧备份（保留最近 10 份）
    fn backup_on_startup(&self) {
        let file = self.dir.join("data.json");
        if !file.exists() {
            return;
        }
        let backups = self.dir.join("backups");
        let name = format!("data-{}.json", stamp());
        let _ = fs::copy(&file, backups.join(name));
        prune_backups(&backups, 10);
    }
}

fn prune_backups(dir: &Path, keep: usize) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    let mut files: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|ext| ext == "json"))
        .collect();
    files.sort(); // 文件名含固定宽度时间戳，字典序 = 时间序
    let remove_count = files.len().saturating_sub(keep);
    for path in files.into_iter().take(remove_count) {
        let _ = fs::remove_file(path);
    }
}

fn stamp() -> String {
    chrono::Local::now().format("%Y%m%d-%H%M%S").to_string()
}

fn data_dir() -> Result<PathBuf, String> {
    // 测试用覆盖：设置 DESKTOP_CALENDAR_DATA_DIR 环境变量可指定数据目录
    if let Ok(dir) = std::env::var("DESKTOP_CALENDAR_DATA_DIR") {
        if !dir.trim().is_empty() {
            return Ok(PathBuf::from(dir));
        }
    }
    let exe = std::env::current_exe().map_err(|e| format!("无法定位程序路径: {e}"))?;
    let dir = exe
        .parent()
        .ok_or_else(|| "无法定位程序目录".to_string())?
        .join("data");
    Ok(dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_event() -> Event {
        Event {
            id: "evt_test".into(),
            title: "验证持久化".into(),
            start_date: "2026-10-20".into(),
            start_time: None,
            end_date: "2026-10-20".into(),
            completed: false,
            completed_at: None,
            remind_enabled: true,
            remind_at_override: None,
            remind_log: Vec::new(),
            source_text: None,
            source_fragment: None,
            recurrence: None,
            recurrence_days_of_week: None,
            recurrence_until: None,
            created_at: "2026-10-15T21:00".into(),
            updated_at: "2026-10-15T21:00".into(),
        }
    }

    /// 验收项：写入 → 丢弃内存 → 重新加载，数据仍在；且启动备份已滚动
    #[test]
    fn persist_across_restart() {
        let tmp = std::env::temp_dir().join(format!("dc-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        // SAFETY: 测试为单线程场景（唯一用例），无并发读写环境变量
        unsafe { std::env::set_var("DESKTOP_CALENDAR_DATA_DIR", &tmp) };

        let store = DataStore::init().expect("首次初始化失败");
        store
            .with_mut(|d| {
                d.events.push(sample_event());
                Ok(())
            })
            .expect("写入失败");
        drop(store); // 模拟退出

        let store2 = DataStore::init().expect("重启加载失败");
        assert_eq!(store2.with(|d| d.events.len()), 1, "重启后事件丢失");
        assert_eq!(
            store2.with(|d| d.events[0].title.clone()),
            "验证持久化".to_string()
        );

        // 第二次启动应已滚出一份备份
        let backups = fs::read_dir(tmp.join("backups")).expect("backups 目录缺失");
        assert!(backups.count() >= 1, "启动备份未生成");

        let _ = fs::remove_dir_all(&tmp);
    }
}
