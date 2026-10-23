//! M2 提醒引擎：到期计算 / 30 秒心跳 / 静默弹窗 / 开机补提醒 / 托盘标记
//!
//! 提醒语义（后端方案 §6，用户原始需求）：
//!   - 默认：活动"最后一天的前一天 07:30"（settings.defaultRemindTime）
//!   - 单条微调：remindAtOverride 支持 "YYYY-MM-DDTHH:MM"（绝对）或 "HH:MM"（endDate 前一天的时刻）
//!   - 只有"未完成"才提醒；已勾完成的不弹
//!   - remindLog 按 plannedAt 去重：记过账的提醒不重复弹
//!   - 关机错过的 → 启动时补弹（catchUpOnStart 开时），记账 status="missed"
//!   - 运行中的 → 心跳 tick 到点即弹，记账 status="fired"
//!   - 过点后才创建的事件（典型：导入历史/当天活动）→ 一律不弹窗防打扰，
//!     只记账 status="late"，仅在应用内提醒面板可见（后端方案 §8）
//!
//! 弹窗走 WinRT Toast（tauri-winrt-notification），不创建 WebView，内存开销近零；
//! AUMID 注册在 HKCU（免管理员），让弹窗显示"桌面日历"而不是"Windows PowerShell"。

use std::time::Duration;

use chrono::{Days, Local, NaiveDate, NaiveDateTime, NaiveTime, Datelike};
use tauri::{AppHandle, Manager};
use tauri_winrt_notification::{Duration as ToastDuration, Toast};

use crate::data::{AppData, DataStore, Event, RemindLogEntry};

const FALLBACK_HHMM: &str = "07:30";
pub const AUMID: &str = "com.desktopcalendar.app";
const TICK_SECS: u64 = 30;

/// 计算单条事件的下一次提醒时刻；None = 不提醒（提醒关闭 / 日期非法）
pub fn planned_at(ev: &Event, default_hhmm: &str) -> Option<NaiveDateTime> {
    if !ev.remind_enabled {
        return None;
    }
    if let Some(raw) = ev.remind_at_override.as_deref() {
        let s = raw.trim();
        if let Ok(dt) = NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M") {
            return Some(dt);
        }
        if s.len() == 5 {
            // "HH:MM"：endDate 前一天的时刻
            if let Some(t) = parse_hhmm(s) {
                return end_minus_one(&ev.end_date).map(|d| d.and_time(t));
            }
        }
        // 覆盖值非法 → 落回默认规则
    }
    let d = end_minus_one(&ev.end_date)?;
    let t = parse_hhmm(default_hhmm)
        .or_else(|| parse_hhmm(FALLBACK_HHMM))
        .expect("兜底时刻常量必须可解析");
    Some(d.and_time(t))
}

fn end_minus_one(end_date: &str) -> Option<NaiveDate> {
    let d = NaiveDate::parse_from_str(end_date, "%Y-%m-%d").ok()?;
    d.checked_sub_days(Days::new(1))
}

fn parse_hhmm(s: &str) -> Option<NaiveTime> {
    NaiveTime::parse_from_str(s.trim(), "%H:%M").ok()
}

pub fn fmt_stamp(p: NaiveDateTime) -> String {
    p.format("%Y-%m-%dT%H:%M").to_string()
}

/// 一条到点待弹的提醒
pub struct DueReminder {
    pub event_id: String,
    pub title: String,
    pub end_date: String,
    pub planned: NaiveDateTime,
    /// 事件创建时间晚于计划提醒时刻（如过点后才导入的活动）→ 不弹窗防打扰，只记账
    pub created_after_planned: bool,
}

/// 展开重复事件：为重复事件生成所有到当前日期为止的出现实例
fn expand_occurrences(ev: &Event, up_to: NaiveDate) -> Vec<String> {
    let recurrence = match ev.recurrence.as_deref() {
        Some(r) if !r.is_empty() => r,
        _ => return vec![ev.end_date.clone()],
    };

    let start = match NaiveDate::parse_from_str(&ev.start_date, "%Y-%m-%d") {
        Ok(d) => d,
        Err(_) => return vec![ev.end_date.clone()],
    };
    let end = match NaiveDate::parse_from_str(&ev.end_date, "%Y-%m-%d") {
        Ok(d) => d,
        Err(_) => return vec![ev.end_date.clone()],
    };
    let span = end.signed_duration_since(start).num_days() as u64;
    let until = ev
        .recurrence_until
        .as_deref()
        .and_then(|s| NaiveDate::parse_from_str(s, "%Y-%m-%d").ok())
        .unwrap_or(up_to)
        .min(up_to);

    let mut dates = Vec::new();

    match recurrence {
        "daily" => {
            let mut current = start;
            while current <= until {
                let occ_end = current.checked_add_days(Days::new(span)).unwrap_or(current);
                dates.push(occ_end.format("%Y-%m-%d").to_string());
                current = match current.checked_add_days(Days::new(1)) {
                    Some(next) => next,
                    None => break,
                };
            }
        }
        "weekly" => {
            let default_dow = start.weekday().num_days_from_sunday() as u8;
            let days: Vec<u8> = ev.recurrence_days_of_week.clone().unwrap_or_else(|| vec![default_dow]);
            let mut current = start;
            while current <= until {
                let wd = current.weekday().num_days_from_sunday() as u8;
                if days.contains(&wd) {
                    let occ_end = current.checked_add_days(Days::new(span)).unwrap_or(current);
                    dates.push(occ_end.format("%Y-%m-%d").to_string());
                }
                current = match current.checked_add_days(Days::new(1)) {
                    Some(next) => next,
                    None => break,
                };
            }
        }
        "monthly" => {
            let day = start.day();
            let mut month_offset = 0u32;
            loop {
                let target = match start.checked_add_months(chrono::Months::new(month_offset)) {
                    Some(d) => d,
                    None => break,
                };
                if target > until { break; }
                let clamped = clamp_day(target.year(), target.month(), day);
                if let Some(occ_start) = clamped {
                    let occ_end = occ_start.checked_add_days(Days::new(span)).unwrap_or(occ_start);
                    dates.push(occ_end.format("%Y-%m-%d").to_string());
                }
                month_offset += 1;
            }
        }
        "yearly" => {
            let month = start.month();
            let day = start.day();
            let mut year_offset = 0i32;
            loop {
                let target_year = start.year() + year_offset;
                if target_year > until.year() { break; }
                if let Some(occ_start) = clamp_day(target_year, month, day) {
                    if occ_start > until { break; }
                    let occ_end = occ_start.checked_add_days(Days::new(span)).unwrap_or(occ_start);
                    dates.push(occ_end.format("%Y-%m-%d").to_string());
                }
                year_offset += 1;
            }
        }
        _ => return vec![ev.end_date.clone()],
    }

    if dates.is_empty() {
        vec![ev.end_date.clone()]
    } else {
        dates
    }
}

fn clamp_day(year: i32, month: u32, day: u32) -> Option<NaiveDate> {
    let max_day = days_in_month(year, month);
    NaiveDate::from_ymd_opt(year, month, day.min(max_day))
}

fn days_in_month(year: i32, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => if is_leap_year(year) { 29 } else { 28 },
        _ => 30,
    }
}

fn is_leap_year(year: i32) -> bool {
    (year % 4 == 0 && year % 100 != 0) || year % 400 == 0
}

/// 已到点、未完成、未记账的提醒清单（含重复事件展开）
pub fn collect_due(data: &AppData, now: NaiveDateTime) -> Vec<DueReminder> {
    let default = data.settings.default_remind_time.clone();
    let today = now.date();
    let mut result = Vec::new();

    for ev in data.events.iter().filter(|e| !e.completed) {
        let occurrences = expand_occurrences(ev, today);

        for occ_end in &occurrences {
            let temp_ev = Event {
                end_date: occ_end.clone(),
                remind_enabled: ev.remind_enabled,
                remind_at_override: ev.remind_at_override.clone(),
                created_at: ev.created_at.clone(),
                ..ev.clone()
            };
            let p = match planned_at(&temp_ev, &default) {
                Some(dt) => dt,
                None => continue,
            };
            if p > now {
                continue;
            }
            let key = fmt_stamp(p);
            if ev.remind_log.iter().any(|l| l.planned_at == key) {
                continue;
            }
            result.push(DueReminder {
                event_id: ev.id.clone(),
                title: ev.title.clone(),
                end_date: occ_end.clone(),
                planned: p,
                created_after_planned: created_after(ev, p),
            });
        }
    }
    result
}

/// 创建时间是否晚于计划时刻（解析失败按"不晚于"处理，即照常提醒）
fn created_after(ev: &Event, planned: NaiveDateTime) -> bool {
    NaiveDateTime::parse_from_str(&ev.created_at, "%Y-%m-%dT%H:%M")
        .map(|c| c > planned)
        .unwrap_or(false)
}

/// 待处理提醒条数：提醒过（弹过 / 补过）但仍未完成的事件 —— 决定托盘红点
/// 对于重复事件，只要有任何一个实例被提醒过且事件未完成，就计数
pub fn pending_count(data: &AppData) -> usize {
    data.events
        .iter()
        .filter(|e| {
            // 已完成的事件不计
            if e.completed {
                return false;
            }
            // 有提醒记录的事件才计（说明至少有一个实例已触发提醒）
            !e.remind_log.is_empty()
        })
        .count()
}

// ———————— 运行期 ————————

/// 启动时调用一次：AUMID 注册 → 补提醒 → 托盘标记 → 起心跳线程
pub fn start(app: &AppHandle) {
    register_aumid();
    catch_up(app);
    refresh_tray(app);

    let handle = app.clone();
    std::thread::spawn(move || loop {
        std::thread::sleep(Duration::from_secs(TICK_SECS));
        tick(&handle);
    });
}

/// 开机补提醒：错过的（已到点未记账）→ 事前安排的记账 missed 并弹汇总；
/// 过点后新建的（created_after_planned）→ 只记账 missed→late，静默进面板
fn catch_up(app: &AppHandle) {
    let store = app.state::<DataStore>();
    let now = Local::now().naive_local();
    let (due, catch_up_on) = store.with(|d| (collect_due(d, now), d.settings.catch_up_on_start));
    if due.is_empty() {
        return;
    }
    let (on_time, late): (Vec<_>, Vec<_>) =
        due.into_iter().partition(|d| !d.created_after_planned);

    append_logs(&store, &late, now, "late");
    append_logs(&store, &on_time, now, "missed");
    if !late.is_empty() {
        log(format!("[reminder] 过点新建 {} 条（静默记账 late）", late.len()));
    }

    if on_time.is_empty() {
        return;
    }
    if catch_up_on {
        let text = if on_time.len() == 1 {
            format!("「{}」的提醒时间已过，任务还未完成", on_time[0].title)
        } else {
            let names: Vec<&str> = on_time.iter().take(3).map(|d| d.title.as_str()).collect();
            format!(
                "有 {} 条提醒已错过：{}{}",
                on_time.len(),
                names.join("、"),
                if on_time.len() > 3 { " 等" } else { "" }
            )
        };
        notify("补提醒 · 错过的活动", &text);
        log(format!("[reminder] 补提醒 {} 条", on_time.len()));
    } else {
        log(format!(
            "[reminder] 错过 {} 条（补提醒已关闭，仅记账）",
            on_time.len()
        ));
    }
}

/// 心跳：到点的弹窗并记账；过点后新建的只记账不弹
fn tick(app: &AppHandle) {
    let store = app.state::<DataStore>();
    let now = Local::now().naive_local();
    let due = store.with(|d| collect_due(d, now));
    if due.is_empty() {
        return;
    }
    let (on_time, late): (Vec<_>, Vec<_>) =
        due.into_iter().partition(|d| !d.created_after_planned);

    append_logs(&store, &late, now, "late");
    append_logs(&store, &on_time, now, "fired");

    let today = now.date();
    for item in &on_time {
        notify("活动提醒 · 桌面日历", &due_message(item, today));
    }
    if !on_time.is_empty() {
        log(format!("[reminder] 弹出 {} 条", on_time.len()));
    }
    if !late.is_empty() {
        log(format!("[reminder] 过点新建 {} 条（静默记账 late）", late.len()));
    }
    refresh_tray(app);
}

fn append_logs(store: &DataStore, due: &[DueReminder], now: NaiveDateTime, status: &str) {
    if due.is_empty() {
        return; // with_mut 无条件落盘，空清单不写盘
    }
    let result = store.with_mut(|d| {
        for item in due {
            if let Some(ev) = d.events.iter_mut().find(|e| e.id == item.event_id) {
                ev.remind_log.push(RemindLogEntry {
                    planned_at: fmt_stamp(item.planned),
                    fired_at: fmt_stamp(now),
                    status: status.to_string(),
                });
            }
        }
        Ok(())
    });
    if let Err(e) = result {
        log(format!("[reminder] 记账失败: {e}"));
    }
}

fn due_message(item: &DueReminder, today: NaiveDate) -> String {
    let end = NaiveDate::parse_from_str(&item.end_date, "%Y-%m-%d").ok();
    match end.map(|e| e.signed_duration_since(today).num_days()) {
        Some(1) => format!("「{}」明天是最后一天，完成后记得勾选", item.title),
        Some(0) => format!("「{}」今天是最后一天，完成后记得勾选", item.title),
        _ => format!("「{}」将于 {} 到期", item.title, item.end_date),
    }
}

/// 静默弹窗（无声音、无 WebView）
fn notify(title: &str, text: &str) {
    let result = Toast::new(AUMID)
        .title(title)
        .text1(text)
        .sound(None)
        .duration(ToastDuration::Short)
        .show();
    if let Err(e) = result {
        log(format!("[reminder] 弹窗失败: {e}"));
    }
}

/// 注册 AUMID（HKCU，幂等，免管理员）：
/// Win32 程序的 WinRT Toast 需要它，弹窗才会显示"桌面日历"而非默认应用名
fn register_aumid() {
    let key = format!(r"HKCU\Software\Classes\AppUserModelId\{AUMID}");
    let result = std::process::Command::new("reg")
        .args(["add", &key, "/v", "DisplayName", "/d", "桌面日历", "/f"])
        .output();
    if let Err(e) = result {
        log(format!("[reminder] AUMID 注册失败: {e}"));
    }
}

/// 托盘标记：有待处理提醒 → 红点图标 + 计数提示；否则恢复默认
pub fn refresh_tray(app: &AppHandle) {
    let Some(tray) = app.tray_by_id("main") else {
        return;
    };
    let pending = app.state::<DataStore>().with(pending_count);
    if pending > 0 {
        if let Ok(img) = tauri::image::Image::from_bytes(include_bytes!("../icons/tray-dot.png")) {
            let _ = tray.set_icon(Some(img));
        }
        let _ = tray.set_tooltip(Some(format!("桌面日历 · {pending} 条提醒待处理")));
    } else {
        if let Some(icon) = app.default_window_icon() {
            let _ = tray.set_icon(Some(icon.clone()));
        }
        let _ = tray.set_tooltip(Some("桌面日历"));
    }
}

/// stdout 输出（GUI 子系统下句柄可能无效，错误一律吞掉，绝不 panic）
fn log(msg: String) {
    use std::io::Write;
    let _ = writeln!(std::io::stdout(), "{msg}");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(id: &str, end: &str, override_: Option<&str>) -> Event {
        Event {
            id: id.into(),
            title: format!("事件{id}"),
            start_date: end.into(),
            start_time: None,
            end_date: end.into(),
            completed: false,
            completed_at: None,
            remind_enabled: true,
            remind_at_override: override_.map(str::to_string),
            remind_log: Vec::new(),
            source_text: None,
            source_fragment: None,
            recurrence: None,
            recurrence_days_of_week: None,
            recurrence_until: None,
            created_at: "2026-01-01T00:00".into(),
            updated_at: "2026-01-01T00:00".into(),
        }
    }

    fn data_with(events: Vec<Event>) -> AppData {
        let mut d = AppData::default();
        d.events = events;
        d
    }

    fn dt(s: &str) -> NaiveDateTime {
        NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M").unwrap()
    }

    fn log_entry(planned: &str) -> RemindLogEntry {
        RemindLogEntry {
            planned_at: planned.into(),
            fired_at: planned.into(),
            status: "fired".into(),
        }
    }

    #[test]
    fn default_rule_is_day_before_at_0730() {
        let e = ev("a", "2026-10-20", None);
        assert_eq!(planned_at(&e, "07:30").unwrap(), dt("2026-10-19T07:30"));
    }

    #[test]
    fn override_absolute_and_hhmm() {
        let a = ev("a", "2026-10-20", Some("2026-10-18T09:15"));
        assert_eq!(planned_at(&a, "07:30").unwrap(), dt("2026-10-18T09:15"));

        let b = ev("b", "2026-10-20", Some("21:00"));
        assert_eq!(planned_at(&b, "07:30").unwrap(), dt("2026-10-19T21:00"));
    }

    #[test]
    fn invalid_override_falls_back_to_default() {
        let e = ev("a", "2026-10-20", Some("不是时间"));
        assert_eq!(planned_at(&e, "08:00").unwrap(), dt("2026-10-19T08:00"));
    }

    #[test]
    fn reminders_disabled_produces_none() {
        let mut e = ev("a", "2026-10-20", None);
        e.remind_enabled = false;
        assert!(planned_at(&e, "07:30").is_none());
    }

    #[test]
    fn due_skips_future_completed_and_logged() {
        let now = dt("2026-10-19T08:00");

        let fired = ev("due", "2026-10-20", None); // 已到点未记账 → 应弹

        let mut done = ev("done", "2026-10-20", None);
        done.completed = true; // 已完成 → 跳过

        let mut logged = ev("logged", "2026-10-20", None);
        logged.remind_log.push(log_entry("2026-10-19T07:30")); // 已记账 → 跳过

        let future = ev("future", "2026-10-30", None); // 未到点 → 跳过

        let data = data_with(vec![fired, done, logged, future]);
        let due = collect_due(&data, now);
        assert_eq!(due.len(), 1);
        assert_eq!(due[0].event_id, "due");

        // 记账后不再重复弹
        let mut again = ev("due", "2026-10-20", None);
        again.remind_log.push(log_entry(&fmt_stamp(dt("2026-10-19T07:30"))));
        assert!(collect_due(&data_with(vec![again]), now).is_empty());
    }

    #[test]
    fn late_created_events_are_flagged() {
        let now = dt("2026-10-19T08:00");

        // 事前安排：10-18 创建，计划 10-19 07:30 → 不标 late
        let mut early = ev("early", "2026-10-20", None);
        early.created_at = "2026-10-18T10:00".into();

        // 过点后新建：10-19 07:40 创建（计划 07:30 已过）→ 标 late
        let mut late = ev("late", "2026-10-20", None);
        late.created_at = "2026-10-19T07:40".into();

        // 创建时间解析失败 → 按不晚于处理（照常提醒）
        let mut bad = ev("bad", "2026-10-20", None);
        bad.created_at = "坏时间".into();

        let due = collect_due(&data_with(vec![early, late, bad]), now);
        let flag = |id: &str| {
            due.iter()
                .find(|d| d.event_id == id)
                .map(|d| d.created_after_planned)
                .unwrap()
        };
        assert!(!flag("early"));
        assert!(flag("late"));
        assert!(!flag("bad"));
    }

    #[test]
    fn pending_counts_reminded_uncompleted() {
        let mut a = ev("a", "2026-10-20", None);
        a.remind_log.push(log_entry("2026-10-19T07:30"));

        let b = ev("b", "2026-10-25", None); // 没提醒过 → 不计

        let mut c = ev("c", "2026-10-20", None);
        c.completed = true;
        c.remind_log.push(log_entry("2026-10-19T07:30"));

        assert_eq!(pending_count(&data_with(vec![a, b, c])), 1);
    }
}
