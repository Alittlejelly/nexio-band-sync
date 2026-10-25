//! 课表 JSON 的解析与归一化
//!
//! 手环端（Nexio课程表 rpk）只接受 v4 整表协议：
//! protocol=nexio.schedule, version=4, action=replace|clear,
//! 含 courses / settings / times / holidays 字段（与手机端 WatchPayload.buildFullJson 同构）。
//! 本模块对用户导入的 JSON 做宽松解析：缺协议字段就补默认值，结构不对就明确报错。

use crate::state::Summary;
use serde_json::{json, Map, Value};

pub const PROTOCOL: &str = "nexio.schedule";

#[derive(Debug)]
pub struct Imported {
    pub json: String,
    pub summary: Summary,
}

pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

pub fn import_json(bytes: &[u8]) -> Result<Imported, String> {
    let text = std::str::from_utf8(bytes).map_err(|_| "文件不是 UTF-8 文本".to_string())?;
    import_str(text.trim_start_matches('\u{feff}'))
}

pub(crate) fn import_str(text: &str) -> Result<Imported, String> {
    let mut root: Value = serde_json::from_str(text).map_err(|e| format!("JSON 解析失败：{e}"))?;
    let obj = root
        .as_object_mut()
        .ok_or_else(|| "顶层必须是 JSON 对象".to_string())?;

    match obj.get("protocol") {
        Some(Value::String(s)) if s == PROTOCOL => {}
        Some(Value::String(other)) => {
            return Err(format!("protocol 不匹配：{other}（需要 {PROTOCOL}）"))
        }
        Some(_) => return Err("protocol 字段必须是字符串".into()),
        None => {
            obj.insert("protocol".into(), json!(PROTOCOL));
        }
    }

    // 手环端唯一支持的协议版本
    obj.insert("version".into(), json!(4));

    let action = obj
        .get("action")
        .and_then(Value::as_str)
        .unwrap_or("replace")
        .to_string();
    let is_clear = action == "clear";
    let action = if is_clear || action == "replace" {
        action
    } else {
        "replace".to_string()
    };
    // 统一写回，保证输出总是带明确的 action
    obj.insert("action".into(), json!(action));

    let mut warnings: Vec<String> = Vec::new();

    let has_courses = matches!(obj.get("courses"), Some(Value::Array(_)));
    if !is_clear && !has_courses {
        return Err(
            "缺少 courses 数组：手环端只支持 v4 整表协议（需含 courses/settings/times/holidays）"
                .into(),
        );
    }
    let courses = obj
        .get("courses")
        .and_then(Value::as_array)
        .map(|a| a.len())
        .unwrap_or(0);
    if !is_clear && courses == 0 {
        warnings.push("courses 是空数组，推送后手环将没有课程".into());
    }

    match obj.get("settings") {
        Some(Value::Object(_)) => {}
        None | Some(Value::Null) => {
            obj.insert("settings".into(), json!({}));
            warnings.push("缺少 settings：没有开学日期，手环无法推算教学周".into());
        }
        Some(_) => return Err("settings 必须是对象".into()),
    }
    if !matches!(obj.get("times"), Some(Value::Object(_))) {
        obj.insert("times".into(), json!({}));
    }
    if !matches!(obj.get("holidays"), Some(Value::Array(_))) {
        obj.insert("holidays".into(), json!([]));
    }

    // schedule_name 兼容 scheduleName 写法
    let name = match obj.get("schedule_name").and_then(Value::as_str) {
        Some(s) if !s.is_empty() => s.to_string(),
        _ => {
            let n = obj
                .get("scheduleName")
                .and_then(Value::as_str)
                .unwrap_or("未命名课表")
                .to_string();
            obj.insert("schedule_name".into(), json!(n.clone()));
            n
        }
    };

    obj.insert("sentAt".into(), json!(now_ms()));

    let settings: Map<String, Value> = obj["settings"].as_object().cloned().unwrap_or_default();
    let class_start_time = settings
        .get("class_start_time")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    if class_start_time.is_empty() {
        warnings.push("settings.class_start_time 缺失".into());
    }
    let total_weeks = fmt_num(settings.get("total_weeks"));
    if total_weeks == "?" {
        warnings.push("settings.total_weeks 缺失".into());
    }
    let current_week = fmt_num(settings.get("current_week"));
    let holidays = obj["holidays"].as_array().map(|a| a.len()).unwrap_or(0);

    // v4.1 逐日裁决覆盖表：透传给手环端，非法形态归一为空数组；
    // 只统计 date 合法（YYYY-MM-DD）的条目数给摘要展示（手环端会自行丢弃非法条目）
    let overrides_count = match obj.get_mut("overrides") {
        Some(Value::Array(list)) => {
            let valid = list
                .iter()
                .filter(|o| {
                    o.get("date")
                        .and_then(Value::as_str)
                        .map(|d| d.len() == 10 && d.as_bytes()[4] == b'-' && d.as_bytes()[7] == b'-')
                        .unwrap_or(false)
                })
                .count();
            valid
        }
        Some(_) => {
            obj.insert("overrides".into(), json!([]));
            0
        }
        None => 0,
    };

    let json = serde_json::to_string(&root).map_err(|e| format!("JSON 序列化失败：{e}"))?;

    Ok(Imported {
        json,
        summary: Summary {
            name,
            courses,
            class_start_time,
            total_weeks,
            current_week,
            holidays,
            overrides: overrides_count,
            warnings,
        },
    })
}

fn fmt_num(v: Option<&Value>) -> String {
    match v {
        Some(Value::Number(n)) => n.to_string(),
        Some(Value::String(s)) if !s.is_empty() => s.clone(),
        _ => "?".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 与手环端 tools/check-wearable-sync.mjs 的 payloadFull() 同构的 v4 整表 fixture
    fn full_v4() -> String {
        serde_json::json!({
            "protocol": "nexio.schedule",
            "version": 4,
            "action": "replace",
            "sentAt": 1760000000000u64,
            "schedule_name": "测试课表",
            "settings": {
                "class_start_time": "2026/09/13",
                "current_week": 4,
                "total_weeks": 18,
                "morning_sections": 6,
                "afternoon_sections": 5,
                "evening_sections": 3
            },
            "times": {
                "morning": { "1": "08:00-08:40", "2": "08:50-09:30" },
                "afternoon": { "1": "14:40-15:20" },
                "evening": { "1": "19:30-20:10" }
            },
            "courses": [
                { "id": "c1", "name": "高等数学", "location": "A101", "teacher": "张老师",
                  "dayOfWeek": 1, "startSection": 1, "endSection": 2, "isCustomTime": false,
                  "customStartTime": "", "customEndTime": "", "startWeek": 2, "endWeek": 12,
                  "weekType": 0, "selectedWeeks": [] }
            ],
            "holidays": [
                { "date": "2026-10-10", "endDate": "", "name": "补班", "type": 1,
                  "followDate": "2026-10-12", "followWeek": 5, "followWeekday": -1, "custom": false }
            ]
        })
        .to_string()
    }

    #[test]
    fn full_v4_passes_through() {
        let out = import_json(full_v4().as_bytes()).unwrap();
        let v: Value = serde_json::from_str(&out.json).unwrap();
        assert_eq!(v["protocol"], "nexio.schedule");
        assert_eq!(v["version"], 4);
        assert_eq!(v["action"], "replace");
        assert!(v["sentAt"].as_u64().unwrap() >= 1760000000000u64);
        assert_eq!(v["courses"].as_array().unwrap().len(), 1);
        let s = &out.summary;
        assert_eq!(s.name, "测试课表");
        assert_eq!(s.courses, 1);
        assert_eq!(s.class_start_time, "2026/09/13");
        assert_eq!(s.total_weeks, "18");
        assert_eq!(s.current_week, "4");
        assert_eq!(s.holidays, 1);
        assert!(s.warnings.is_empty());
    }

    #[test]
    fn missing_envelope_fields_are_filled() {
        let body = r#"{
            "scheduleName": "我的课表",
            "settings": { "class_start_time": "2026/09/13", "total_weeks": 18 },
            "courses": []
        }"#;
        let out = import_json(body.as_bytes()).unwrap();
        let v: Value = serde_json::from_str(&out.json).unwrap();
        assert_eq!(v["protocol"], "nexio.schedule");
        assert_eq!(v["version"], 4);
        assert_eq!(v["action"], "replace");
        assert_eq!(v["schedule_name"], "我的课表");
        assert_eq!(v["times"], serde_json::json!({}));
        assert_eq!(v["holidays"], serde_json::json!([]));
        assert!(out.summary.warnings.iter().any(|w| w.contains("courses")));
    }

    #[test]
    fn wrong_protocol_rejected() {
        let bad = r#"{ "protocol": "other.app", "courses": [] }"#;
        assert!(import_json(bad.as_bytes()).is_err());
    }

    #[test]
    fn replace_without_courses_rejected() {
        let bad = r#"{ "protocol": "nexio.schedule", "settings": {} }"#;
        let err = import_json(bad.as_bytes()).unwrap_err();
        assert!(err.contains("courses"));
    }

    #[test]
    fn clear_action_allowed_without_courses() {
        let body = r#"{ "protocol": "nexio.schedule", "action": "clear" }"#;
        let out = import_json(body.as_bytes()).unwrap();
        let v: Value = serde_json::from_str(&out.json).unwrap();
        assert_eq!(v["action"], "clear");
    }

    #[test]
    fn missing_settings_warns() {
        let body = r#"{ "protocol": "nexio.schedule", "courses": [ { "id": "c1" } ] }"#;
        let out = import_json(body.as_bytes()).unwrap();
        assert!(out
            .summary
            .warnings
            .iter()
            .any(|w| w.contains("class_start_time") || w.contains("settings")));
    }

    #[test]
    fn overrides_passthrough_v41() {
        let body = r#"{
            "protocol": "nexio.schedule",
            "settings": { "class_start_time": "2026/09/13", "total_weeks": 18 },
            "courses": [ { "id": "c1" } ],
            "overrides": [
                { "date": "2026-10-12", "holiday": false, "weekday": 1, "week": 5, "courses": [] },
                { "date": "2026-10-01", "holiday": true, "holidayName": "国庆节" },
                { "bad": "entry" }
            ]
        }"#;
        let out = import_json(body.as_bytes()).unwrap();
        let v: Value = serde_json::from_str(&out.json).unwrap();
        // 原样透传给手环（手环端会自行丢弃 date 不合法的条目）
        assert_eq!(v["overrides"].as_array().unwrap().len(), 3);
        // 摘要只统计 date 合法的条目
        assert_eq!(out.summary.overrides, 2);
        assert!(out
            .summary
            .detail_lines()
            .iter()
            .any(|l| l.contains("逐日覆盖：2 天")));
    }

    #[test]
    fn invalid_overrides_normalized_to_empty() {
        let body = r#"{
            "protocol": "nexio.schedule",
            "settings": {},
            "courses": [],
            "overrides": "oops"
        }"#;
        let out = import_json(body.as_bytes()).unwrap();
        let v: Value = serde_json::from_str(&out.json).unwrap();
        assert_eq!(v["overrides"], serde_json::json!([]));
        assert_eq!(out.summary.overrides, 0);
    }

    #[test]
    fn missing_overrides_ok() {
        let out = import_json(full_v4().as_bytes()).unwrap();
        let v: Value = serde_json::from_str(&out.json).unwrap();
        assert!(v.get("overrides").is_none());
        assert_eq!(out.summary.overrides, 0);
    }

    #[test]
    fn invalid_json_rejected() {
        assert!(import_json(b"{ not json").is_err());
    }

    #[test]
    fn bom_is_stripped() {
        let mut bytes = vec![0xEF, 0xBB, 0xBF];
        bytes.extend_from_slice(full_v4().as_bytes());
        assert!(import_json(&bytes).is_ok());
    }
}
