//! IPC 命令层（接口契约见后端方案 §12）
//!
//! 命名对照：
//!   events_list         → events.list({from,to})
//!   events_create       → events.create(payload)
//!   events_update       → events.update(id, patch)
//!   events_remove       → events.remove(id)
//!   events_set_completed→ events.setCompleted(id, bool)
//!   settings_get/update → settings.get/update(patch)
//!   app_info            → 调试用（数据目录、版本、构建类型）

use serde_json::{json, Value};
use tauri::{AppHandle, State};

use crate::data::{DataStore, Event, Settings};
use crate::reminder;

fn now_iso() -> String {
    chrono::Local::now().format("%Y-%m-%dT%H:%M").to_string()
}

fn new_event_id() -> String {
    format!("evt_{}", uuid::Uuid::new_v4().simple())
}

fn is_valid_date(s: &str) -> bool {
    chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d").is_ok()
}

/// 把 patch 对象里"白名单内"的键合并进 base（未列出的键一律忽略）
fn merge_known_fields(base: &mut Value, patch: &Value, allowed: &[&str]) {
    if let (Value::Object(base_map), Value::Object(patch_map)) = (base, patch) {
        for key in allowed {
            if let Some(v) = patch_map.get(*key) {
                base_map.insert((*key).to_string(), v.clone());
            }
        }
    }
}

// ———————— events ————————

#[tauri::command]
pub fn events_list(
    store: State<'_, DataStore>,
    from: Option<String>,
    to: Option<String>,
) -> Result<Vec<Event>, String> {
    let list = store.with(|d| {
        d.events
            .iter()
            .filter(|e| {
                // 区间重叠：事件 [startDate, endDate] 与查询 [from, to] 有交集
                // （ISO 日期字符串可直接字典序比较）
                from.as_deref().is_none_or(|f| e.end_date.as_str() >= f)
                    && to.as_deref().is_none_or(|t| e.start_date.as_str() <= t)
            })
            .cloned()
            .collect::<Vec<_>>()
    });
    Ok(list)
}

#[tauri::command]
pub fn events_create(store: State<'_, DataStore>, payload: Value) -> Result<Event, String> {
    let title = payload
        .get("title")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_string();
    if title.is_empty() {
        return Err("标题不能为空".into());
    }

    let start_date = payload
        .get("startDate")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    if !is_valid_date(&start_date) {
        return Err("开始日期必须是 YYYY-MM-DD".into());
    }

    let end_date = payload
        .get("endDate")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .unwrap_or(&start_date)
        .to_string();
    if !is_valid_date(&end_date) {
        return Err("结束日期必须是 YYYY-MM-DD".into());
    }
    if end_date < start_date {
        return Err("结束日期不能早于开始日期".into());
    }

    let now = now_iso();
    let event = Event {
        id: new_event_id(),
        title,
        start_date,
        start_time: payload
            .get("startTime")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_string),
        end_date,
        completed: false,
        completed_at: None,
        remind_enabled: payload
            .get("remindEnabled")
            .and_then(Value::as_bool)
            .unwrap_or(true),
        remind_at_override: payload
            .get("remindAtOverride")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_string),
        remind_log: Vec::new(),
        source_text: payload
            .get("sourceText")
            .and_then(Value::as_str)
            .map(str::to_string),
        source_fragment: payload
            .get("sourceFragment")
            .and_then(Value::as_str)
            .map(str::to_string),
        // M6.2 重复事件
        recurrence: payload
            .get("recurrence")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_string),
        recurrence_days_of_week: payload
            .get("recurrenceDaysOfWeek")
            .and_then(Value::as_array)
            .map(|arr| arr.iter().filter_map(|v| v.as_u64().map(|n| n as u8)).collect()),
        recurrence_until: payload
            .get("recurrenceUntil")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_string),
        created_at: now.clone(),
        updated_at: now,
    };

    let created = event.clone();
    store.with_mut(|d| {
        d.events.push(event);
        Ok(())
    })?;
    Ok(created)
}

#[tauri::command]
pub fn events_update(store: State<'_, DataStore>, id: String, patch: Value) -> Result<Event, String> {
    const ALLOWED: &[&str] = &[
        "title",
        "startDate",
        "startTime",
        "endDate",
        "completed",
        "remindEnabled",
        "remindAtOverride",
        "sourceText",
        "sourceFragment",
        "recurrence",
        "recurrenceDaysOfWeek",
        "recurrenceUntil",
    ];

    store.with_mut(|d| {
        let idx = d
            .events
            .iter()
            .position(|e| e.id == id)
            .ok_or_else(|| format!("事件不存在: {id}"))?;

        let mut current = serde_json::to_value(&d.events[idx]).map_err(|e| e.to_string())?;
        merge_known_fields(&mut current, &patch, ALLOWED);
        let mut updated: Event =
            serde_json::from_value(current).map_err(|e| format!("字段不合法: {e}"))?;

        if !is_valid_date(&updated.start_date) || !is_valid_date(&updated.end_date) {
            return Err("日期必须是 YYYY-MM-DD".into());
        }
        if updated.end_date < updated.start_date {
            return Err("结束日期不能早于开始日期".into());
        }

        updated.updated_at = now_iso();
        d.events[idx] = updated.clone();
        Ok(updated)
    })
}

#[tauri::command]
pub fn events_remove(
    app: AppHandle,
    store: State<'_, DataStore>,
    id: String,
) -> Result<(), String> {
    store.with_mut(|d| {
        let before = d.events.len();
        d.events.retain(|e| e.id != id);
        if d.events.len() == before {
            return Err(format!("事件不存在: {id}"));
        }
        Ok(())
    })?;
    reminder::refresh_tray(&app);
    Ok(())
}

#[tauri::command]
pub fn events_set_completed(
    app: AppHandle,
    store: State<'_, DataStore>,
    id: String,
    completed: bool,
) -> Result<Event, String> {
    let ev = store.with_mut(|d| {
        let ev = d
            .events
            .iter_mut()
            .find(|e| e.id == id)
            .ok_or_else(|| format!("事件不存在: {id}"))?;
        ev.completed = completed;
        ev.completed_at = if completed { Some(now_iso()) } else { None };
        ev.updated_at = now_iso();
        Ok(ev.clone())
    })?;
    reminder::refresh_tray(&app);
    Ok(ev)
}

// ———————— settings ————————

#[tauri::command]
pub fn settings_get(store: State<'_, DataStore>) -> Settings {
    store.with(|d| d.settings.clone())
}

#[tauri::command]
pub fn settings_update(
    app: AppHandle,
    store: State<'_, DataStore>,
    patch: Value,
) -> Result<Settings, String> {
    const ALLOWED: &[&str] = &[
        "defaultRemindTime",
        "catchUpOnStart",
        "aiParseEnabled",
        "autoLaunch",
        "widget",
        "sources",
    ];

    let (old_auto_launch, merged) = store.with_mut(|d| {
        let old = d.settings.auto_launch;
        let mut current = serde_json::to_value(&d.settings).map_err(|e| e.to_string())?;
        merge_known_fields(&mut current, &patch, ALLOWED);
        let settings: Settings =
            serde_json::from_value(current).map_err(|e| format!("字段不合法: {e}"))?;
        d.settings = settings.clone();
        Ok((old, settings))
    })?;

    // 自启开关变了 → 同步到系统（HKCU Run）
    if merged.auto_launch != old_auto_launch {
        sync_autostart(&app, merged.auto_launch);
    }
    Ok(merged)
}

fn sync_autostart(app: &AppHandle, enable: bool) {
    use tauri_plugin_autostart::ManagerExt;
    let manager = app.autolaunch();
    let result = if enable {
        manager.enable()
    } else {
        manager.disable()
    };
    if let Err(e) = result {
        eprintln!("[autostart] 同步失败(enable={enable}): {e}");
    }
}

// ———————— 提醒状态（M2 调试台展示）————————

#[tauri::command]
pub fn reminders_status(store: State<'_, DataStore>) -> Value {
    let now = chrono::Local::now().naive_local();
    store.with(|d| {
        let next = d
            .events
            .iter()
            .filter(|e| !e.completed)
            .filter_map(|e| reminder::planned_at(e, &d.settings.default_remind_time))
            .filter(|p| *p > now)
            .min()
            .map(reminder::fmt_stamp);
        json!({
            "pending": reminder::pending_count(d),
            "nextPlannedAt": next,
        })
    })
}

// ———————— M3 文本解析 ————————

/// 解析粘贴文本 → 预览候选（规则引擎，纯离线）
/// 额外做"与已有活动日期重叠"提示（仅提醒，不阻止导入）
#[tauri::command]
pub fn parse_text(
    store: State<'_, DataStore>,
    text: String,
) -> Result<Vec<crate::parser::ParsedEvent>, String> {
    if text.trim().is_empty() {
        return Err("请输入要解析的文本".into());
    }
    let mut items = crate::parser::parse_text(&text);
    items.truncate(50); // 防误贴超长文本
    store.with(|d| {
        for it in items.iter_mut() {
            if it.start_date.is_empty() {
                continue;
            }
            if let Some(ev) = d.events.iter().find(|e| {
                e.start_date.as_str() <= it.end_date.as_str()
                    && e.end_date.as_str() >= it.start_date.as_str()
            }) {
                let span = if ev.end_date != ev.start_date {
                    format!("{}~{}", ev.start_date, ev.end_date)
                } else {
                    ev.start_date.clone()
                };
                it.warnings
                    .push(format!("与已有活动「{}」（{}）日期重叠，请核对", ev.title, span));
            }
        }
    });
    Ok(items)
}

// ———————— 调试信息 ————————

#[tauri::command]
pub fn app_info(store: State<'_, DataStore>) -> Value {
    json!({
        "version": env!("CARGO_PKG_VERSION"),
        "profile": if cfg!(debug_assertions) { "dev" } else { "release" },
        "dataDir": store.dir.display().to_string(),
        "dataFile": store.dir.join("data.json").display().to_string(),
    })
}

// ———————— M4 主题 ————————

#[tauri::command]
pub fn theme_extract_colors(image_bytes: Vec<u8>) -> Result<Value, String> {
    let tokens = crate::theme::extract_theme_from_bytes(&image_bytes)?;
    Ok(serde_json::to_value(tokens).unwrap())
}

#[tauri::command]
pub fn theme_save(store: State<'_, DataStore>, tokens: Value) -> Result<(), String> {
    store.with_mut(|d| {
        d.settings.current_theme = Some(serde_json::from_value(tokens).unwrap());
        Ok(())
    })
}

#[tauri::command]
pub fn theme_load(store: State<'_, DataStore>) -> Result<Option<Value>, String> {
    let theme = store.with(|d| d.settings.current_theme.clone());
    Ok(theme.map(|t| serde_json::to_value(t).unwrap()))
}

// ———————— M6 OCR ————————

/// 从图片文件路径 OCR 提取文字
#[tauri::command]
pub fn ocr_extract_text(image_path: String) -> Result<String, String> {
    crate::ocr::ocr_file(&image_path)
}

/// 从图片字节 OCR 提取文字（前端 FileReader 读文件后直传）
#[tauri::command]
pub fn ocr_extract_bytes(image_bytes: Vec<u8>) -> Result<String, String> {
    // 写入临时文件后走文件 OCR（WinRT OCR 需要文件路径）
    let temp_dir = std::env::temp_dir().join("desktop_calendar_ocr");
    std::fs::create_dir_all(&temp_dir).map_err(|e| format!("创建临时目录失败: {e}"))?;
    let temp_path = temp_dir.join(format!("ocr_{}.png", uuid::Uuid::new_v4().simple()));
    std::fs::write(&temp_path, &image_bytes).map_err(|e| format!("写临时文件失败: {e}"))?;
    let result = crate::ocr::ocr_file(temp_path.to_str().unwrap_or_default());
    let _ = std::fs::remove_file(&temp_path);
    result
}

/// 从剪贴板读取图片并 OCR
#[tauri::command]
pub fn ocr_clipboard_text() -> Result<String, String> {
    crate::ocr::ocr_clipboard()
}
