//! M3 文本解析引擎（规则版）：切分 → 归一化 → 抽取 → 校验 → 预览候选
//!
//! 管线（后端方案 §7）：
//!   文本 → ① 切分（换行/句读/序号，"一话多条"）→ ② 归一化（全角转半角）→
//!   ③ 抽取（日期/时间/标题）→ ④ 校验（缺年份取最近未来、12月→1月跨年、括号星期对齐）→
//!   ⑤ ParsedEvent 预览候选（逐字段可改、逐条勾选，由前端完成）
//!
//! 覆盖设计 §7.2 的常见写法；未覆盖（按 §7.5 交预览卡手动处理）：
//! 农历/节日名、"旬/半夜"等模糊说法、同一行不加标点的两条活动。
//! 本模块纯离线、零网络依赖。

use std::sync::LazyLock;

use chrono::{Datelike, Days, Local, NaiveDate};
use regex::Regex;
use serde::Serialize;

/// 解析候选（预览卡数据）
#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ParsedEvent {
    pub title: String,
    /// "" = 日期待定（未识别到日期）
    pub start_date: String,
    /// "HH:MM"；None = 全天
    pub start_time: Option<String>,
    pub end_date: String,
    pub warnings: Vec<String>,
    /// 原文片段（留痕，可回看/重解析）
    pub fragment: String,
}

pub fn parse_text(text: &str) -> Vec<ParsedEvent> {
    parse_with_today(text, Local::now().date_naive())
}

/// 可注入"今天"的主入口（单测用固定日期，保证可回归）
pub fn parse_with_today(text: &str, today: NaiveDate) -> Vec<ParsedEvent> {
    let norm = normalize(text);
    let units = split_units(&norm, today);
    let items: Vec<ParsedEvent> = units.iter().map(|u| parse_unit(u, today)).collect();
    if items.is_empty() {
        vec![empty_parsed(&norm)]
    } else {
        items
    }
}

fn empty_parsed(text: &str) -> ParsedEvent {
    ParsedEvent {
        title: "未命名活动".into(),
        start_date: String::new(),
        start_time: None,
        end_date: String::new(),
        warnings: vec!["未识别到日期，已放入「日期待定」区，可手动补".into()],
        fragment: text.trim().chars().take(120).collect(),
    }
}

// ———————— ② 归一化：全角 → 半角 ————————

fn normalize(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 8);
    for c in s.chars() {
        let mapped = match c {
            '０'..='９' => char::from_u32(c as u32 - 0xFF10 + '0' as u32).unwrap_or(c),
            '：' => ':',
            '；' => ';',
            '，' => ',',
            '．' => '.',
            '！' => '!',
            '？' => '?',
            '～' => '~',
            '－' => '-',
            '\u{3000}' => ' ',
            _ => c,
        };
        out.push(mapped);
    }
    out
}

// ———————— ① 切分 ————————

static RE_SPLIT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[。！？!?;；\n\r]+").unwrap());
static RE_MARKER: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\d{1,2}[、.．)）]").unwrap());

fn has_date(s: &str, today: NaiveDate) -> bool {
    !date_tokens(s, today).is_empty()
}

/// 句内的进一步切分：
///   a) 「、/,」两侧都是真日期才切开（"10月19日、20日"不拆，是范围表述）
///   b) 行内序号 "2、" 后紧跟日期 → 在序号处切开（"1、…2、…"同行的情形）
fn refine_split(coarse: &str, today: NaiveDate) -> Vec<String> {
    let mut starts: Vec<usize> = Vec::new();

    let chars: Vec<(usize, char)> = coarse.char_indices().collect();
    let mut prev_end = 0usize;
    for (idx, &(i, c)) in chars.iter().enumerate() {
        if c != '、' && c != ',' {
            continue;
        }
        let next_start = i + c.len_utf8();
        if next_start <= prev_end {
            continue;
        }
        let left = &coarse[prev_end..i];
        let right_end = chars[idx + 1..]
            .iter()
            .find(|(_, cc)| *cc == '、' || *cc == ',')
            .map(|(j, _)| *j)
            .unwrap_or(coarse.len());
        let right = &coarse[next_start..right_end];
        if has_date(left, today) && has_date(right, today) {
            starts.push(next_start);
            prev_end = next_start;
        }
    }

    for m in RE_MARKER.find_iter(coarse) {
        let after = &coarse[m.end()..];
        let ws = after.len() - after.trim_start().len();
        let tok_at = m.end() + ws;
        if tok_at >= coarse.len() {
            continue;
        }
        let toks = date_tokens(&coarse[tok_at..], today);
        if toks.first().is_some_and(|t| t.start == 0) {
            starts.push(m.start());
        }
    }

    starts.sort_unstable();
    starts.dedup();

    let mut segs: Vec<String> = Vec::new();
    let mut s = 0usize;
    for &b in &starts {
        if b > s {
            segs.push(coarse[s..b].to_string());
        }
        s = b;
    }
    if s < coarse.len() {
        segs.push(coarse[s..].to_string());
    }
    if segs.is_empty() {
        segs.push(coarse.to_string());
    }
    segs
}

/// 切成解析单元：强边界（换行/句读）→ 句内细切 → 无日期子句并入邻近带日期子句
fn split_units(text: &str, today: NaiveDate) -> Vec<String> {
    let mut all: Vec<String> = Vec::new();
    for coarse in RE_SPLIT.split(text) {
        let coarse = coarse.trim();
        if coarse.is_empty() {
            continue;
        }
        let mut cur = String::new();
        let mut cur_has = false;
        for seg in refine_split(coarse, today) {
            let seg = seg.trim();
            if seg.is_empty() || is_marker_only(seg, today) {
                continue;
            }
            let has = has_date(seg, today);
            if cur.is_empty() {
                cur = seg.to_string();
                cur_has = has;
            } else if has && cur_has {
                all.push(std::mem::take(&mut cur));
                cur = seg.to_string();
                cur_has = true;
            } else {
                cur.push(' ');
                cur.push_str(seg);
                cur_has = cur_has || has;
            }
        }
        if !cur.is_empty() {
            all.push(cur);
        }
    }

    // 全文没有任何日期 → 整体当一条"日期待定"
    if !all.iter().any(|u| has_date(u, today)) {
        return vec![text.trim().to_string()];
    }

    // 跨句合并：不带日期的碎句并入前一条；若在最前则并入下一条
    let mut out: Vec<String> = Vec::new();
    let mut pending = String::new();
    for u in all {
        if has_date(&u, today) {
            if pending.is_empty() {
                out.push(u);
            } else {
                pending.push(' ');
                pending.push_str(&u);
                out.push(std::mem::take(&mut pending));
            }
        } else if out.is_empty() {
            if !pending.is_empty() {
                pending.push(' ');
            }
            pending.push_str(&u);
        } else if let Some(last) = out.last_mut() {
            last.push(' ');
            last.push_str(&u);
        }
    }
    out
}

/// 整段只是序号/标点（如 "1"、"2）"），丢弃
fn is_marker_only(s: &str, today: NaiveDate) -> bool {
    !s.is_empty()
        && s.chars().all(|c| {
            c.is_ascii_digit() || matches!(c, '.' | '、' | ')' | '）' | '(' | '（' | '-' | ' ' | '．')
        })
        && date_tokens(s, today).is_empty()
}

// ———————— ③ 抽取：日期 ————————

#[derive(Debug, Clone, Copy, PartialEq)]
enum DateKind {
    Ymd,
    Md,
    Dot,
    Week,
    Rel,
}

#[derive(Debug, Clone)]
struct DateTok {
    start: usize,
    end: usize,
    kind: DateKind,
    m: u32,
    d: u32,
    /// 0=周一 … 6=周日（Week 专用）
    week_num: Option<u32>,
    resolved: Option<NaiveDate>,
}

static RE_YMD: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(\d{4})年(\d{1,2})月(\d{1,2})[日号]").unwrap());
static RE_MD: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(\d{1,2})月(\d{1,2})[日号]").unwrap());
static RE_DOT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(\d{1,2})\.(\d{1,2})").unwrap());
static RE_WEEK: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(本|这|下下|下|上)?(周|星期|礼拜)([一二三四五六日天1-7])").unwrap()
});
static RE_REL: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(大后天|后天|明天|今天)").unwrap());

fn date_tokens(s: &str, today: NaiveDate) -> Vec<DateTok> {
    let mut toks: Vec<DateTok> = Vec::new();

    for c in RE_YMD.captures_iter(s) {
        let (y, m, d) = (
            c[1].parse::<i32>().unwrap_or(0),
            c[2].parse::<u32>().unwrap_or(0),
            c[3].parse::<u32>().unwrap_or(0),
        );
        let Some(date) = NaiveDate::from_ymd_opt(y, m, d) else { continue };
        let full = c.get(0).unwrap();
        toks.push(DateTok {
            start: full.start(),
            end: full.end(),
            kind: DateKind::Ymd,
            m,
            d,
            week_num: None,
            resolved: Some(date),
        });
    }

    for c in RE_MD.captures_iter(s) {
        let full = c.get(0).unwrap();
        // 已被"年+月日"整段覆盖的跳过
        if toks.iter().any(|t| {
            t.kind == DateKind::Ymd && t.start <= full.start() && t.end >= full.end()
        }) {
            continue;
        }
        let (m, d) = (
            c[1].parse::<u32>().unwrap_or(0),
            c[2].parse::<u32>().unwrap_or(0),
        );
        if !(1..=12).contains(&m) || !(1..=31).contains(&d) {
            continue;
        }
        // 月份天数合法性用闰年 2000 兜底检查（2月29日 合法）
        if NaiveDate::from_ymd_opt(2000, m, d).is_none() {
            continue;
        }
        toks.push(DateTok {
            start: full.start(),
            end: full.end(),
            kind: DateKind::Md,
            m,
            d,
            week_num: None,
            resolved: None,
        });
    }

    let bytes = s.as_bytes();
    for c in RE_DOT.captures_iter(s) {
        let full = c.get(0).unwrap();
        let (b0, b1) = (full.start(), full.end());
        let bad_before = b0 > 0 && (bytes[b0 - 1].is_ascii_digit() || bytes[b0 - 1] == b'.');
        let bad_after = b1 < bytes.len() && (bytes[b1].is_ascii_digit() || bytes[b1] == b'.');
        if bad_before || bad_after {
            continue;
        }
        let (m, d) = (
            c[1].parse::<u32>().unwrap_or(0),
            c[2].parse::<u32>().unwrap_or(0),
        );
        if !(1..=12).contains(&m) || NaiveDate::from_ymd_opt(2000, m, d).is_none() {
            continue;
        }
        toks.push(DateTok {
            start: b0,
            end: b1,
            kind: DateKind::Dot,
            m,
            d,
            week_num: None,
            resolved: None,
        });
    }

    for c in RE_WEEK.captures_iter(s) {
        let full = c.get(0).unwrap();
        let wd = match &c[3] {
            "一" | "1" => 0,
            "二" | "2" => 1,
            "三" | "3" => 2,
            "四" | "4" => 3,
            "五" | "5" => 4,
            "六" | "6" => 5,
            "日" | "天" | "7" => 6,
            _ => continue,
        };
        let prefix = c.get(1).map(|x| x.as_str());
        let resolved = resolve_week(prefix, wd, today);
        toks.push(DateTok {
            start: full.start(),
            end: full.end(),
            kind: DateKind::Week,
            m: 0,
            d: 0,
            week_num: Some(wd),
            resolved: Some(resolved),
        });
    }

    for c in RE_REL.captures_iter(s) {
        let full = c.get(0).unwrap();
        let off = match full.as_str() {
            "今天" => 0,
            "明天" => 1,
            "后天" => 2,
            "大后天" => 3,
            _ => continue,
        };
        toks.push(DateTok {
            start: full.start(),
            end: full.end(),
            kind: DateKind::Rel,
            m: 0,
            d: 0,
            week_num: None,
            resolved: Some(today + Days::new(off)),
        });
    }

    toks.sort_by_key(|t| t.start);
    toks
}

fn resolve_week(prefix: Option<&str>, wd: u32, today: NaiveDate) -> NaiveDate {
    let today_idx = today.weekday().num_days_from_monday();
    let monday = today - Days::new(today_idx as u64);
    let base = monday + Days::new(wd as u64);
    match prefix {
        Some("下") => base + Days::new(7),
        Some("下下") => base + Days::new(14),
        Some("上") => base - Days::new(7),
        Some("本") | Some("这") => base,
        // 光杆"周X"：取最近未来（含今天）
        _ => {
            if base < today {
                base + Days::new(7)
            } else {
                base
            }
        }
    }
}

fn pick_primary(toks: &[DateTok]) -> Option<&DateTok> {
    for kind in [
        DateKind::Ymd,
        DateKind::Md,
        DateKind::Dot,
        DateKind::Week,
        DateKind::Rel,
    ] {
        if let Some(t) = toks
            .iter()
            .filter(|t| t.kind == kind)
            .min_by_key(|t| t.start)
        {
            return Some(t);
        }
    }
    None
}

/// 校验 + 定年份：无年份取最近未来；带括号星期时优先按星期对齐定年份
fn resolve_primary(
    t: &DateTok,
    all: &[DateTok],
    today: NaiveDate,
) -> Option<(NaiveDate, Option<String>)> {
    match t.kind {
        DateKind::Ymd | DateKind::Rel => {
            let d = t.resolved?;
            let warn = week_mismatch_warn(d, all);
            Some((d, warn))
        }
        DateKind::Week => Some((t.resolved?, None)),
        DateKind::Md | DateKind::Dot => {
            let (m, d) = (t.m, t.d);
            let ty = today.year();
            // 设计口径：无年份一律取"最近未来"；括号星期只做校验（不符则警告）
            let date = [ty, ty + 1]
                .iter()
                .filter_map(|&y| NaiveDate::from_ymd_opt(y, m, d))
                .find(|dt| *dt >= today)?;
            let warn = week_mismatch_warn(date, all);
            Some((date, warn))
        }
    }
}

fn week_mismatch_warn(date: NaiveDate, all: &[DateTok]) -> Option<String> {
    let hint = all
        .iter()
        .find(|x| x.kind == DateKind::Week)
        .and_then(|x| x.week_num)?;
    (date.weekday().num_days_from_monday() != hint)
        .then(|| "文字里的星期与日期对不上，请核对日期".to_string())
}

// ———————— ③ 抽取：范围（10月20日到23日） ————————

static RE_DAYONLY: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(\d{1,2})[日号]").unwrap());

fn range_end(
    unit: &str,
    after: usize,
    start: NaiveDate,
    today: NaiveDate,
) -> Option<(NaiveDate, (usize, usize))> {
    let rest = &unit[after..];
    let trimmed = rest.trim_start();
    let p0 = after + (rest.len() - trimmed.len());
    let conn = ["到", "至", "~", "-", "—", "–", "、", ","]
        .iter()
        .find(|c| unit[p0..].starts_with(*c))?;
    let p1 = p0 + conn.len();
    let rest2 = &unit[p1..];
    let trimmed2 = rest2.trim_start();
    let p2 = p1 + (rest2.len() - trimmed2.len());
    let sub = &unit[p2..];
    if sub.is_empty() {
        return None;
    }

    // 完整日期优先："10月20日到10月23日" / "12月28日至1月3日"
    if let Some(t) = date_tokens(sub, today).first() {
        if t.start <= 2 {
            let end = resolve_range_end_tok(t, start, today)?;
            return Some((end, (after, p2 + t.end)));
        }
    }
    // "至23日"：日份继承 start 的月份/年份
    if let Some(c) = RE_DAYONLY.captures(sub) {
        let f = c.get(0).unwrap();
        if f.start() <= 2 {
            let day: u32 = c[1].parse().ok()?;
            let end = resolve_dayonly(day, start)?;
            return Some((end, (after, p2 + f.end())));
        }
    }
    None
}

fn resolve_range_end_tok(t: &DateTok, start: NaiveDate, today: NaiveDate) -> Option<NaiveDate> {
    match t.kind {
        DateKind::Ymd | DateKind::Rel => t.resolved,
        DateKind::Week => {
            let r = t.resolved?;
            Some(if r < start { r + Days::new(7) } else { r })
        }
        DateKind::Md | DateKind::Dot => {
            let base = [today.year(), today.year() + 1]
                .iter()
                .filter_map(|&y| NaiveDate::from_ymd_opt(y, t.m, t.d))
                .find(|d| *d >= today)?;
            if base < start {
                NaiveDate::from_ymd_opt(base.year() + 1, t.m, t.d)
            } else {
                Some(base)
            }
        }
    }
}

fn resolve_dayonly(day: u32, start: NaiveDate) -> Option<NaiveDate> {
    if day >= 1 {
        if let Some(d) = NaiveDate::from_ymd_opt(start.year(), start.month(), day) {
            if d >= start {
                return Some(d);
            }
        }
    }
    let (y, m) = if start.month() == 12 {
        (start.year() + 1, 1)
    } else {
        (start.year(), start.month() + 1)
    };
    NaiveDate::from_ymd_opt(y, m, day)
}

// ———————— ③ 抽取：时间 ————————

/// 时段词（长的在前，便于最长匹配）
const PERIODS: &[&str] = &[
    "凌晨", "早上", "早晨", "上午", "中午", "下午", "傍晚", "晚上", "晚间", "夜里", "晚", "早",
];

static RE_TIME_HM: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(\d{1,2}):(\d{2})").unwrap());
static RE_TIME_CN: LazyLock<Regex> = LazyLock::new(|| {
    // 注意：单字"早/晚"不进 h 前缀（否则"早一点/晚一点"会被误吞），靠 lookback 补判
    Regex::new(
        r"(凌晨|早上|早晨|上午|中午|下午|傍晚|晚上|晚间|夜里)?\s*([0-9]{1,2}|[零一二两三四五六七八九十]{1,3})\s*[点时](半|一刻|三刻|[0-9]{1,2}分)?",
    )
    .unwrap()
});

struct TimeOut {
    /// (HH:MM, Some(原始文本)=未说明上午下午的歧义小时)
    chosen: Option<(String, Option<String>)>,
    spans: Vec<(usize, usize)>,
    period_only: bool,
}

fn extract_time(s: &str) -> TimeOut {
    // (start, h, m, period, raw)
    let mut cands: Vec<(usize, u32, u32, Option<String>, String)> = Vec::new();
    let mut spans: Vec<(usize, usize)> = Vec::new();

    for c in RE_TIME_HM.captures_iter(s) {
        let f = c.get(0).unwrap();
        let (h, m) = (
            c[1].parse::<u32>().unwrap_or(99),
            c[2].parse::<u32>().unwrap_or(99),
        );
        if h > 23 || m > 59 {
            continue;
        }
        let (span_start, period) = match lookback_period(s, f.start()) {
            Some((st, p)) => (st, Some(p)),
            None => (f.start(), None),
        };
        cands.push((f.start(), h, m, period, f.as_str().to_string()));
        spans.push((span_start, f.end()));
    }

    for c in RE_TIME_CN.captures_iter(s) {
        let f = c.get(0).unwrap();
        let num = c.get(2).unwrap().as_str();
        let is_cn = !num.chars().next().unwrap().is_ascii_digit();
        let h = if is_cn {
            cn2num(num)
        } else {
            num.parse::<u32>().ok()
        };
        let Some(h) = h else { continue };
        if h > 23 {
            continue;
        }
        let m = match c.get(3).map(|x| x.as_str()) {
            Some("半") => 30,
            Some("一刻") => 15,
            Some("三刻") => 45,
            Some(x) => x.trim_end_matches('分').parse::<u32>().unwrap_or(0),
            None => 0,
        };
        if m > 59 {
            continue;
        }
        let (span_start, period) = if let Some(p) = c.get(1) {
            (f.start(), Some(p.as_str().to_string()))
        } else {
            match lookback_period(s, f.start()) {
                Some((st, p)) => (st, Some(p)),
                None => (f.start(), None),
            }
        };
        // 过滤"非时刻"的中文数字用法：一点点 / 第一点 / 一点建议 / 三点要求
        if c.get(3).is_none() && is_cn {
            let p = period.as_deref();
            if p.is_none() {
                if num == "一" {
                    continue; // 光杆"一点"基本是"早一点/一点建议"，不当时刻
                }
                let prev = s[..f.start()].chars().last();
                if matches!(prev, Some('第') | Some('哪') | Some('这') | Some('那')) {
                    continue;
                }
                let after = &s[f.end()..];
                if ["意见", "建议", "要求", "事项", "注意", "好处", "坏处", "理由", "心得", "感受"]
                    .iter()
                    .any(|w| after.starts_with(w))
                {
                    continue;
                }
            } else if matches!(p, Some("早") | Some("晚")) && num == "一" {
                continue; // "早一点/晚一点"口语；"晚7点"这类带具体数字的仍保留
            }
        }
        cands.push((f.start(), h, m, period, f.as_str().to_string()));
        spans.push((span_start, f.end()));
    }

    // 合并重叠 span
    spans.sort_unstable();
    let mut merged: Vec<(usize, usize)> = Vec::new();
    for sp in spans {
        if let Some(last) = merged.last_mut() {
            if sp.0 <= last.1 {
                last.1 = last.1.max(sp.1);
                continue;
            }
        }
        merged.push(sp);
    }

    cands.sort_by_key(|c| c.0);
    if let Some((_, h, m, period, raw)) = cands.first() {
        let (hh, mm) = finalize_time(*h, *m, period.as_deref());
        let ambiguous = period.is_none() && (1..=6).contains(h);
        TimeOut {
            chosen: Some((
                format!("{hh:02}:{mm:02}"),
                ambiguous.then(|| raw.clone()),
            )),
            spans: merged,
            period_only: false,
        }
    } else {
        TimeOut {
            chosen: None,
            spans: merged,
            period_only: contains_period_word(s),
        }
    }
}

fn lookback_period(s: &str, pos: usize) -> Option<(usize, String)> {
    let win = &s[..pos];
    for p in PERIODS {
        if win.ends_with(p) {
            return Some((pos - p.len(), (*p).to_string()));
        }
    }
    None
}

fn finalize_time(h: u32, m: u32, period: Option<&str>) -> (u32, u32) {
    let h2 = match period {
        Some("中午") => {
            if h == 12 {
                12
            } else if h <= 5 {
                h + 12
            } else {
                h
            }
        }
        Some("下午") | Some("傍晚") => {
            if h < 12 {
                h + 12
            } else {
                h
            }
        }
        Some("晚上") | Some("晚间") | Some("夜里") | Some("晚") => {
            if h < 12 {
                h + 12
            } else {
                h
            }
        }
        _ => h,
    };
    (h2.min(23), m)
}

/// 无具体时刻但出现时段词（"明天下午 交材料"）
fn contains_period_word(s: &str) -> bool {
    for p in ["凌晨", "早上", "早晨", "上午", "中午", "下午", "傍晚", "晚上", "晚间", "夜里"] {
        if s.contains(p) {
            return true;
        }
    }
    for (i, c) in s.char_indices() {
        if c == '晚' || c == '早' {
            let next = s[i + c.len_utf8()..].chars().next();
            if !matches!(
                next,
                Some('会') | Some('餐') | Some('点') | Some('到') | Some('些') | Some('日')
                    | Some('安') | Some('上') | Some('间') | Some('一')
            ) {
                return true;
            }
        }
    }
    false
}

fn cn2num(s: &str) -> Option<u32> {
    let cs: Vec<char> = s.chars().collect();
    let d = |c: char| match c {
        '零' => Some(0),
        '一' => Some(1),
        '二' | '两' => Some(2),
        '三' => Some(3),
        '四' => Some(4),
        '五' => Some(5),
        '六' => Some(6),
        '七' => Some(7),
        '八' => Some(8),
        '九' => Some(9),
        '十' => Some(10),
        _ => None,
    };
    match cs.len() {
        1 => d(cs[0]),
        2 => {
            if cs[0] == '十' {
                d(cs[1]).map(|x| 10 + x)
            } else if cs[1] == '十' {
                d(cs[0]).map(|x| x * 10)
            } else {
                None
            }
        }
        3 => {
            if cs[1] == '十' {
                Some(d(cs[0])? * 10 + d(cs[2])?)
            } else {
                None
            }
        }
        _ => None,
    }
}

// ———————— ④⑤ 单元 → 预览候选 ————————

fn parse_unit(unit: &str, today: NaiveDate) -> ParsedEvent {
    let unit = unit.trim();
    let toks = date_tokens(unit, today);
    let mut warnings: Vec<String> = Vec::new();
    let mut consumed: Vec<(usize, usize)> = toks.iter().map(|t| (t.start, t.end)).collect();

    let mut start_date = String::new();
    let mut end_date = String::new();

    if let Some(primary) = pick_primary(&toks) {
        if let Some((sd, warn)) = resolve_primary(primary, &toks, today) {
            if let Some(w) = warn {
                warnings.push(w);
            }
            if sd < today {
                warnings.push(format!("日期已过去（{}），请核对年份", fmt_date(sd)));
            }
            let mut ed = sd;
            if let Some((e, cspan)) = range_end(unit, primary.end, sd, today) {
                ed = e;
                consumed.push(cspan);
            }
            start_date = fmt_date(sd);
            end_date = fmt_date(ed);
        } else {
            warnings.push("识别到的日期不合法，请手动选择日期".into());
        }
    } else {
        warnings.push("未识别到日期，已放入「日期待定」区，可手动补".into());
    }

    // 时间
    let t = extract_time(unit);
    consumed.extend(t.spans.iter().copied());
    let mut start_time = None;
    if let Some((hhmm, amb_raw)) = t.chosen {
        start_time = Some(hhmm.clone());
        if let Some(raw) = amb_raw {
            let raw = raw.trim().to_string();
            warnings.push(format!("「{raw}」未说明上午/下午，暂按 {hhmm} 处理，可修改"));
        }
    } else if t.period_only {
        warnings.push("只说到时段（如「下午」）未到点，已按全天处理，可补具体时刻".into());
    }

    // 标题
    let leftover = build_leftover(unit, &consumed);
    let title = clean_title(&leftover);
    let title = if title.is_empty() {
        warnings.push("未识别出活动名，请手动填写".into());
        "未命名活动".to_string()
    } else {
        title
    };

    ParsedEvent {
        title,
        start_date,
        start_time,
        end_date,
        warnings,
        fragment: unit.chars().take(120).collect(),
    }
}

/// 把"已消费"的片段剔除，剩下原文交给标题清洗
fn build_leftover(unit: &str, consumed: &[(usize, usize)]) -> String {
    let chars: Vec<char> = unit.chars().collect();
    let mut mask = vec![false; chars.len()];
    for &(bs, be) in consumed {
        let bs = bs.min(unit.len());
        let be = be.min(unit.len());
        let cs = unit[..bs].chars().count();
        let ce = unit[..be].chars().count();
        for m in mask.iter_mut().take(ce).skip(cs) {
            *m = true;
        }
    }
    chars
        .iter()
        .enumerate()
        .filter(|(i, _)| !mask[*i])
        .map(|(_, c)| *c)
        .collect()
}

static RE_EMPTY_BR: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[（(]\s*[）)]").unwrap());
static RE_LEAD_NUM: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s*[（(]?\d{1,2}[、.．)）]\s*").unwrap());

fn clean_title(raw: &str) -> String {
    let mut s = raw.to_string();
    for _ in 0..3 {
        s = RE_EMPTY_BR.replace_all(&s, "").to_string();
    }
    s = s.split_whitespace().collect::<Vec<_>>().join(" ");

    const LEAD_WORDS: &[&str] = &[
        "各位同学", "各位", "同学们", "大家好", "大家", "通知", "公告", "关于", "注意",
        "温馨提示", "特此", "定于", "拟于", "兹定于",
    ];
    const TRAIL_WORDS: &[&str] = &[
        "请准时参加", "准时参加", "请按时参加", "按时参加", "请相互转告", "相互转告",
        "请知悉", "收到请回复", "谢谢大家", "谢谢", "特此通知", "请查收", "勿忘", "周知",
        "记得参加", "请务必参加", "务必参加",
    ];
    let edge_punct = " \t,，.。、;；:：!！?？()（）-—~～的了吧嘛哦";

    for _ in 0..4 {
        let mut t = s.trim_matches(|c| edge_punct.contains(c)).to_string();
        for w in LEAD_WORDS {
            if let Some(rest) = t.strip_prefix(w) {
                t = rest.trim_start().to_string();
            }
        }
        for w in TRAIL_WORDS {
            if let Some(rest) = t.strip_suffix(w) {
                t = rest.trim_end().to_string();
            }
        }
        if t == s {
            break;
        }
        s = t;
    }
    s = RE_LEAD_NUM.replace(&s, "").to_string();
    s = s.trim_matches(|c| edge_punct.contains(c)).to_string();

    if s.chars().count() > 40 {
        s = s.chars().take(40).collect::<String>() + "…";
    }
    s.trim().to_string()
}

fn fmt_date(d: NaiveDate) -> String {
    d.format("%Y-%m-%d").to_string()
}

// ———————— 测试（案例集，作为规则引擎的回归基准） ————————
//
// 基准"今天"：2026-10-15（周四）——10-18 周日、10-19 周一、10-21 周三。

#[cfg(test)]
mod tests {
    use super::*;

    fn today() -> NaiveDate {
        let t = NaiveDate::from_ymd_opt(2026, 10, 15).unwrap();
        assert_eq!(t.weekday().num_days_from_monday(), 3, "基准日必须是周四");
        t
    }

    fn parse(text: &str) -> Vec<ParsedEvent> {
        parse_with_today(text, today())
    }

    fn one(text: &str) -> ParsedEvent {
        let v = parse(text);
        assert_eq!(v.len(), 1, "期望 1 条，实际 {} 条：{v:?}", v.len());
        v.into_iter().next().unwrap()
    }

    #[test]
    fn case_bracket_weekday_evening() {
        // 案例：10月19日（周一）晚7点团日活动
        let it = one("10月19日（周一）晚7点团日活动");
        assert_eq!(it.start_date, "2026-10-19");
        assert_eq!(it.start_time.as_deref(), Some("19:00"));
        assert_eq!(it.end_date, "2026-10-19");
        assert_eq!(it.title, "团日活动");
        assert!(it.warnings.is_empty(), "不应有警告: {:?}", it.warnings);
    }

    #[test]
    fn case_next_week_afternoon_hm() {
        // 案例：下周三下午2:30 部门例会
        let it = one("下周三下午2:30 部门例会");
        assert_eq!(it.start_date, "2026-10-21");
        assert_eq!(it.start_time.as_deref(), Some("14:30"));
        assert_eq!(it.title, "部门例会");
        assert!(it.warnings.is_empty());
    }

    #[test]
    fn case_full_range_and_short_range() {
        // 案例：10月20日到10月23日 校运动会
        let a = one("10月20日到10月23日 校运动会");
        assert_eq!((a.start_date.as_str(), a.end_date.as_str()), ("2026-10-20", "2026-10-23"));
        assert_eq!(a.title, "校运动会");
        assert_eq!(a.start_time, None);

        // 案例：10月20日-23日 志愿活动（"至23日"简写）
        let b = one("10月20日-23日 志愿活动");
        assert_eq!((b.start_date.as_str(), b.end_date.as_str()), ("2026-10-20", "2026-10-23"));
        assert_eq!(b.title, "志愿活动");
    }

    #[test]
    fn case_cross_year_range() {
        // 案例：12月28日至1月3日 寒假实践活动（自动跨年）
        let it = one("12月28日至1月3日 寒假实践活动");
        assert_eq!(it.start_date, "2026-12-28");
        assert_eq!(it.end_date, "2027-01-03");
        assert_eq!(it.title, "寒假实践活动");
    }

    #[test]
    fn case_relative_days_and_cn_time() {
        // 案例：明天下午3点半 交材料
        let a = one("明天下午3点半 交材料");
        assert_eq!(a.start_date, "2026-10-16");
        assert_eq!(a.start_time.as_deref(), Some("15:30"));
        assert_eq!(a.title, "交材料");

        // 案例：后天下午三点半 交材料（中文数字）
        let b = one("后天下午三点半 交材料");
        assert_eq!(b.start_date, "2026-10-17");
        assert_eq!(b.start_time.as_deref(), Some("15:30"));
    }

    #[test]
    fn case_bare_weekday_morning() {
        // 案例：周一早上9点 升旗仪式（今天周四 → 最近的周一）
        let it = one("周一早上9点 升旗仪式");
        assert_eq!(it.start_date, "2026-10-19");
        assert_eq!(it.start_time.as_deref(), Some("09:00"));
        assert_eq!(it.title, "升旗仪式");
    }

    #[test]
    fn case_allday_and_this_sunday() {
        // 案例：10月25日 大扫除（全天，无时段提示）
        let a = one("10月25日 大扫除");
        assert_eq!(a.start_date, "2026-10-25");
        assert_eq!(a.start_time, None);
        assert!(a.warnings.is_empty(), "全天不应警告: {:?}", a.warnings);

        // 案例：本周日（10月18日）下午14:00 体测
        let b = one("本周日（10月18日）下午14:00 体测");
        assert_eq!(b.start_date, "2026-10-18");
        assert_eq!(b.start_time.as_deref(), Some("14:00"));
        assert_eq!(b.title, "体测");
        assert!(b.warnings.is_empty());
    }

    #[test]
    fn case_numbered_two_lines() {
        // 案例：1、10月20日团日活动\n2、10月25日志愿活动 → 2 条
        let v = parse("1、10月20日团日活动\n2、10月25日志愿活动");
        assert_eq!(v.len(), 2);
        assert_eq!(v[0].start_date, "2026-10-20");
        assert_eq!(v[0].title, "团日活动");
        assert_eq!(v[1].start_date, "2026-10-25");
        assert_eq!(v[1].title, "志愿活动");
    }

    #[test]
    fn case_comma_split_same_line() {
        // 同句两条：逗号两侧都是真日期才拆
        let v = parse("10月19日 团日活动，10月25日 志愿活动");
        assert_eq!(v.len(), 2);
        assert_eq!(v[0].title, "团日活动");
        assert_eq!(v[1].title, "志愿活动");
    }

    #[test]
    fn case_no_date_pending() {
        // 案例：交材料（无日期）→ 日期待定
        let it = one("交材料");
        assert_eq!(it.start_date, "");
        assert_eq!(it.end_date, "");
        assert_eq!(it.title, "交材料");
        assert!(it.warnings.iter().any(|w| w.contains("未识别到日期")));
    }

    #[test]
    fn case_dot_date_and_fullwidth() {
        // 案例：10.20 交材料
        let a = one("10.20 交材料");
        assert_eq!(a.start_date, "2026-10-20");

        // 案例：全角数字 １０月２０日 开会
        let b = one("１０月２０日 开会");
        assert_eq!(b.start_date, "2026-10-20");
        assert_eq!(b.title, "开会");
    }

    #[test]
    fn case_period_only_warns() {
        // 案例：明天下午 交材料（只说到时段）
        let it = one("明天下午 交材料");
        assert_eq!(it.start_date, "2026-10-16");
        assert_eq!(it.start_time, None);
        assert!(it.warnings.iter().any(|w| w.contains("只说到时段")));
    }

    #[test]
    fn case_ambiguous_hour_warns() {
        let it = one("10月18日 6:30 集合");
        assert_eq!(it.start_time.as_deref(), Some("06:30"));
        assert!(it.warnings.iter().any(|w| w.contains("未说明上午/下午")));
    }

    #[test]
    fn case_week_mismatch_warns() {
        // 10月20日实际是周二，括号里写周三 → 警告但保留日期
        let it = one("10月20日（周三）开会");
        assert_eq!(it.start_date, "2026-10-20");
        assert!(it.warnings.iter().any(|w| w.contains("对不上")));
    }

    #[test]
    fn case_past_year_warns() {
        let it = one("2025年10月20日 开会");
        assert_eq!(it.start_date, "2025-10-20");
        assert!(it.warnings.iter().any(|w| w.contains("已过去")));
    }

    #[test]
    fn case_real_world_notice() {
        let text = "各位同学：\n大家好！本周日（10月18日）下午14:00 体测。\n10月25日 大扫除，请准时参加。";
        let v = parse(text);
        assert_eq!(v.len(), 2, "{v:?}");
        assert_eq!(v[0].start_date, "2026-10-18");
        assert_eq!(v[0].start_time.as_deref(), Some("14:00"));
        assert_eq!(v[0].title, "体测");
        assert_eq!(v[1].start_date, "2026-10-25");
        assert_eq!(v[1].title, "大扫除");
    }

    #[test]
    fn case_false_time_words_ignored() {
        // "早一点/一点点/第3点"不做时刻
        let it = one("10月18日 大家早一点到，不要迟到");
        assert_eq!(it.start_time, None);
        assert!(it.warnings.iter().all(|w| !w.contains("01:00")));
    }

    #[test]
    fn case_yue_range_kept_together() {
        // "10月19日、20日"是范围表述，不拆成两卡
        let it = one("10月19日、20日 值日");
        assert_eq!(it.start_date, "2026-10-19");
        assert_eq!(it.end_date, "2026-10-20");
        assert_eq!(it.title, "值日");
    }
}
