//! 极简 ICS（iCalendar）解析：把 WakeUp 课表等应用导出的课程事件
//! 转成 Nexio v4 整表 payload（再走 schedule::import_str 归一化）。
//!
//! 支持：VEVENT 的 SUMMARY / LOCATION / DTSTART / DTEND、
//! RRULE(FREQ=WEEKLY, BYDAY, UNTIL, COUNT, INTERVAL)。
//! 局限：UTC(Z) 时间按本地时间处理；EXDATE 忽略；全天事件跳过。

use serde_json::{json, Map, Value};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Date {
    pub y: i32,
    pub m: u32,
    pub d: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct DateTime {
    pub date: Date,
    /// (hour, minute)
    pub hm: (u32, u32),
}

#[derive(Debug, Clone)]
struct Event {
    summary: String,
    location: String,
    start: DateTime,
    end: Option<(u32, u32)>,
    /// RRULE BYDAY：1=周一..7=周日；空 = 用 start 当天
    byday: Vec<u8>,
    until: Option<Date>,
    count: Option<u32>,
    interval: u32,
    has_rrule: bool,
}

// ---------------------------------------------------------------- 日期工具

/// Howard Hinnant days_from_civil：公历日期 → 距 1970-01-01 的天数
fn days_from_civil(y: i32, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y } as i64;
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m as i64 + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

fn civil_from_days(z: i64) -> Date {
    let z = z + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    Date {
        y: (y + if m <= 2 { 1 } else { 0 }) as i32,
        m,
        d,
    }
}

/// 星期几，1=周一..7=周日
fn weekday(date: Date) -> u8 {
    (((days_from_civil(date.y, date.m, date.d) + 3) % 7 + 7) % 7) as u8 + 1
}

/// 该日期所在周的周一
fn monday_of(date: Date) -> Date {
    let days = days_from_civil(date.y, date.m, date.d) - (weekday(date) as i64 - 1);
    civil_from_days(days)
}

fn week_index(date: Date, semester_monday: Date) -> i64 {
    (days_from_civil(date.y, date.m, date.d) - days_from_civil(semester_monday.y, semester_monday.m, semester_monday.d)) / 7 + 1
}

fn today() -> Date {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    civil_from_days(secs.div_euclid(86400))
}

fn fmt_date(d: Date) -> String {
    format!("{:04}/{:02}/{:02}", d.y, d.m, d.d)
}

fn fmt_hm(hm: (u32, u32)) -> String {
    format!("{:02}:{:02}", hm.0, hm.1)
}

fn parse_hm(h: u32, m: u32) -> (u32, u32) {
    (h.min(23), m.min(59))
}

// ---------------------------------------------------------------- ICS 解析

/// 解析 "20260914T080000" / "20260914T080000Z" / "20260914"；
/// 返回 None 表示全天事件或无法解析
fn parse_dt(value: &str) -> Option<DateTime> {
    let v = value.trim().trim_end_matches('Z');
    let (dpart, tpart) = match v.split_once('T') {
        Some((d, t)) => (d, Some(t)),
        None => (v, None),
    };
    if dpart.len() != 8 || !dpart.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let date = Date {
        y: dpart[0..4].parse().ok()?,
        m: dpart[4..6].parse().ok()?,
        d: dpart[6..8].parse().ok()?,
    };
    let hm = match tpart {
        Some(t) => {
            let t: String = t.chars().filter(|c| c.is_ascii_digit()).collect();
            if t.len() < 4 {
                return None;
            }
            parse_hm(t[0..2].parse().ok()?, t[2..4].parse().ok()?)
        }
        None => return None, // 全天事件：不是课程
    };
    Some(DateTime { date, hm })
}

const BYDAY: [(&str, u8); 7] = [
    ("MO", 1),
    ("TU", 2),
    ("WE", 3),
    ("TH", 4),
    ("FR", 5),
    ("SA", 6),
    ("SU", 7),
];

fn parse_byday(s: &str) -> Vec<u8> {
    s.split(',')
        .filter_map(|d| {
            let d = d.trim().trim_start_matches(['+', '-']);
            BYDAY.iter().find(|(k, _)| *k == d).map(|(_, v)| *v)
        })
        .collect()
}

fn unfold(text: &str) -> Vec<(String, String)> {
    // 折行续行以空格/Tab 开头；属性形如 NAME;PARAM=..:VALUE
    let mut raw_lines: Vec<String> = Vec::new();
    for line in text.lines() {
        if line.starts_with(' ') || line.starts_with('\t') {
            if let Some(last) = raw_lines.last_mut() {
                last.push_str(line.trim_start());
                continue;
            }
        }
        raw_lines.push(line.trim_end_matches('\r').to_string());
    }
    raw_lines
        .into_iter()
        .filter_map(|l| {
            let (name, value) = l.split_once(':')?;
            let prop = name.split(';').next()?.trim().to_ascii_uppercase();
            Some((prop, value.trim().to_string()))
        })
        .collect()
}

fn parse_events(text: &str) -> Result<Vec<Event>, String> {
    let mut events = Vec::new();
    let mut cur: Option<Event> = None;
    let mut rrule_raw = String::new();

    for (prop, value) in unfold(text) {
        match prop.as_str() {
            "BEGIN" if value == "VEVENT" => {
                cur = Some(Event {
                    summary: String::new(),
                    location: String::new(),
                    start: DateTime {
                        date: Date { y: 0, m: 0, d: 0 },
                        hm: (0, 0),
                    },
                    end: None,
                    byday: vec![],
                    until: None,
                    count: None,
                    interval: 1,
                    has_rrule: false,
                });
                rrule_raw.clear();
            }
            "END" if value == "VEVENT" => {
                if let Some(mut e) = cur.take() {
                    if !rrule_raw.is_empty() {
                        e.has_rrule = true;
                        for kv in rrule_raw.split(';') {
                            if let Some((k, v)) = kv.split_once('=') {
                                match k.to_ascii_uppercase().as_str() {
                                    "BYDAY" => e.byday = parse_byday(v),
                                    "INTERVAL" => e.interval = v.parse().unwrap_or(1).max(1),
                                    "COUNT" => e.count = v.parse().ok(),
                                    "UNTIL" => e.until = parse_dt(v).map(|dt| dt.date),
                                    _ => {}
                                }
                            }
                        }
                    }
                    if e.start.date.y > 0 {
                        events.push(e);
                    }
                }
            }
            "SUMMARY" => {
                if let Some(e) = cur.as_mut() {
                    e.summary = value.trim().to_string();
                }
            }
            "LOCATION" => {
                if let Some(e) = cur.as_mut() {
                    e.location = value.trim().to_string();
                }
            }
            "DTSTART" => {
                if let Some(e) = cur.as_mut() {
                    if let Some(dt) = parse_dt(&value) {
                        e.start = dt;
                    }
                }
            }
            "DTEND" => {
                if let Some(e) = cur.as_mut() {
                    e.end = parse_dt(&value).map(|dt| dt.hm);
                }
            }
            "RRULE" => rrule_raw = value.trim().to_string(),
            _ => {}
        }
    }

    if events.is_empty() {
        return Err("没有找到带具体时间的课程事件（全天事件会被跳过）".into());
    }
    Ok(events)
}

// ---------------------------------------------------------------- 转 v4 整表

/// 把 ICS 文本转成 v4 整表 payload JSON（随后交给 schedule::import_str 归一化）
pub fn ics_to_payload(text: &str, file_name: &str) -> Result<Value, String> {
    let events = parse_events(text)?;

    // 学期开始：最早事件所在周的周一
    let semester = monday_of(
        events
            .iter()
            .map(|e| e.start.date)
            .min()
            .ok_or("事件日期为空")?,
    );

    // 汇总所有节次起点（按上午/下午/晚上分段）
    let mut starts: Vec<(u32, u32)> = events.iter().map(|e| e.start.hm).collect();
    starts.sort();
    starts.dedup();
    let period_of = |hm: (u32, u32)| -> u8 {
        if hm.0 < 12 {
            0
        } else if hm.0 < 18 {
            1
        } else {
            2
        }
    };
    let section_index = |hm: (u32, u32)| -> Option<usize> {
        starts.iter().position(|s| period_of(*s) == period_of(hm) && *s <= hm)
    };

    // times 表：每段内相邻起点相接
    let mut times_periods: [Map<String, Value>; 3] = Default::default();
    for (pi, _period_name) in ["morning", "afternoon", "evening"].iter().enumerate() {
        let in_period: Vec<&(u32, u32)> = starts.iter().filter(|s| period_of(**s) as usize == pi).collect();
        for (i, s) in in_period.iter().enumerate() {
            let end = match in_period.get(i + 1) {
                Some(next) => {
                    let mins = next.0 * 60 + next.1 - 1;
                    (mins / 60, mins % 60)
                }
                None => {
                    // 段内最后 one：取该起点事件的最晚结束，否则 +45 分钟
                    let max_end = events
                        .iter()
                        .filter(|e| e.start.hm == **s)
                        .filter_map(|e| e.end)
                        .max();
                    max_end.unwrap_or((s.0 + (s.1 + 45) / 60, (s.1 + 45) % 60))
                }
            };
            times_periods[pi].insert((i + 1).to_string(), json!(format!("{}-{}", fmt_hm(**s), fmt_hm(end))));
        }
    }

    // 课程：一个 VEVENT 的每个 BYDAY 各生成一条；周次用 selectedWeeks 表达
    let mut courses: Vec<Value> = Vec::new();
    let mut max_week = 1i64;
    for (ei, e) in events.iter().enumerate() {
        let days: Vec<u8> = if e.byday.is_empty() {
            vec![weekday(e.start.date)]
        } else {
            e.byday.clone()
        };
        let w0 = week_index(e.start.date, semester);
        for &day in &days {
            let selected: Vec<i64> = if !e.has_rrule {
                vec![w0.max(1)]
            } else {
                let last = match (e.until, e.count) {
                    (Some(u), _) => week_index(u, semester),
                    (None, Some(c)) => w0 + (c as i64 - 1) * e.interval as i64,
                    (None, None) => w0 + 24, // 无终止：默认 25 周
                }
                .max(w0);
                let mut weeks: Vec<i64> = Vec::new();
                let mut emitted = 0u32;
                let mut w = w0.max(1);
                while w <= last && (e.count.is_none() || emitted < e.count.unwrap_or(u32::MAX)) {
                    if (w - w0) % e.interval as i64 == 0 {
                        weeks.push(w);
                        emitted += 1;
                    }
                    w += 1;
                }
                if weeks.is_empty() {
                    weeks.push(w0.max(1));
                }
                weeks
            };
            if let Some(&mw) = selected.iter().max() {
                max_week = max_week.max(mw);
            }

            let start_section = section_index(e.start.hm).map(|i| i + 1).unwrap_or(1);
            let end_section = e
                .end
                .and_then(section_index)
                .map(|i| i + 1)
                .unwrap_or(start_section)
                .max(start_section);
            courses.push(json!({
                "id": format!("ics{ei}_{day}"),
                "name": if e.summary.is_empty() { "未命名课程".to_string() } else { e.summary.clone() },
                "location": e.location,
                "teacher": "",
                "dayOfWeek": day,
                "startSection": start_section,
                "endSection": end_section,
                "isCustomTime": true,
                "customStartTime": fmt_hm(e.start.hm),
                "customEndTime": fmt_hm(e.end.unwrap_or(e.start.hm)),
                "startWeek": selected.first().unwrap_or(&1),
                "endWeek": selected.last().unwrap_or(&1),
                "weekType": 0,
                "selectedWeeks": selected,
            }));
        }
    }

    let tw = max_week.max(1);
    let today_w = week_index(today(), semester).max(1);

    let mut times = Map::new();
    times.insert("morning".into(), Value::Object(times_periods[0].clone()));
    times.insert("afternoon".into(), Value::Object(times_periods[1].clone()));
    times.insert("evening".into(), Value::Object(times_periods[2].clone()));

    Ok(json!({
        "schedule_name": file_stem(file_name),
        "settings": {
            "class_start_time": fmt_date(semester),
            "current_week": today_w.min(tw),
            "total_weeks": tw,
            "morning_sections": times_periods[0].len(),
            "afternoon_sections": times_periods[1].len(),
            "evening_sections": times_periods[2].len(),
        },
        "times": Value::Object(times),
        "courses": courses,
        "holidays": [],
    }))
}

fn file_stem(name: &str) -> String {
    let base = name.rsplit(['\\', '/']).next().unwrap_or(name);
    base.split('.').next().unwrap_or(base).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\nSUMMARY:高等数学\r\nLOCATION:A101\r\nDTSTART:20260914T080000\r\nDTEND:20260914T093000\r\nRRULE:FREQ=WEEKLY;BYDAY=MO,WE;UNTIL=20261102T235959Z\r\nEND:VEVENT\r\nBEGIN:VEVENT\r\nSUMMARY:单次讲座\r\nLOCATION:报告厅\r\nDTSTART:20260916T190000\r\nDTEND:20260916T203000\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";

    #[test]
    fn parses_weekly_rrule_into_selected_weeks() {
        let v = ics_to_payload(SAMPLE, "我的课表.ics").unwrap();
        assert_eq!(v["settings"]["class_start_time"], "2026/09/14");
        let courses = v["courses"].as_array().unwrap();
        // 数学（MO+WE 两条）+ 讲座（周四? 不对：9/16 是周三）
        assert_eq!(courses.len(), 3);
        let math_mo = courses.iter().find(|c| c["dayOfWeek"] == 1).unwrap();
        // 2026/09/14 起每周一，直到 11/02（第 8 周）
        let weeks: Vec<i64> = math_mo["selectedWeeks"]
            .as_array()
            .unwrap()
            .iter()
            .map(|w| w.as_i64().unwrap())
            .collect();
        assert_eq!(weeks, vec![1, 2, 3, 4, 5, 6, 7, 8]);
        assert_eq!(math_mo["startSection"], 1);
        assert_eq!(math_mo["isCustomTime"], true);
        assert_eq!(math_mo["customStartTime"], "08:00");
        // 讲座：单次（第 1 周），晚上节次
        let talk = courses.iter().find(|c| c["name"] == "单次讲座").unwrap();
        assert_eq!(talk["selectedWeeks"], serde_json::json!([1]));
        assert!(talk["startSection"].as_u64().unwrap() >= 1);
    }

    #[test]
    fn total_weeks_follows_latest() {
        let v = ics_to_payload(SAMPLE, "x.ics").unwrap();
        assert_eq!(v["settings"]["total_weeks"], 8);
    }

    #[test]
    fn all_day_events_skipped() {
        let ics = "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nSUMMARY:运动会\r\nDTSTART;VALUE=DATE:20261001\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
        assert!(ics_to_payload(ics, "x.ics").is_err());
    }

    #[test]
    fn weekday_math() {
        // 2026-09-14 是周一，1970-01-01 是周四
        assert_eq!(weekday(Date { y: 2026, m: 9, d: 14 }), 1);
        assert_eq!(weekday(Date { y: 1970, m: 1, d: 1 }), 4);
        assert_eq!(fmt_date(monday_of(Date { y: 2026, m: 9, d: 16 })), "2026/09/14");
    }
}
