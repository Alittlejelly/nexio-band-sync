use std::sync::{Mutex, OnceLock};

/// 导入课表的摘要信息（用于 UI 展示）
#[derive(Debug)]
pub struct Summary {
    pub name: String,
    pub courses: usize,
    pub class_start_time: String,
    pub total_weeks: String,
    pub current_week: String,
    pub holidays: usize,
    pub warnings: Vec<String>,
}

impl Summary {
    pub fn detail_lines(&self) -> Vec<String> {
        let mut lines = vec![
            format!("课表名称：{}", self.name),
            format!("课程数量：{} 门", self.courses),
            format!(
                "学期开始：{}",
                if self.class_start_time.is_empty() {
                    "未填写"
                } else {
                    &self.class_start_time
                }
            ),
            format!("总周数：{}　当前周：{}", self.total_weeks, self.current_week),
            format!("假期 / 调休：{} 条", self.holidays),
        ];
        for w in &self.warnings {
            lines.push(format!("⚠ {w}"));
        }
        lines
    }
}

/// 要导入的课表文件格式
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileType {
    Json,
    Ics,
}

pub struct AppState {
    pub root_element_id: Option<String>,
    /// 归一化后可直接推送给手环的 v4 整表 JSON
    pub schedule_json: Option<String>,
    pub summary: Option<Summary>,
    /// 导入的文件名（有则展示在信息卡第一行）
    pub file_name: Option<String>,
    pub import_status: String,
    pub push_status: String,
    /// 当前选择的文件格式
    pub file_type: FileType,
    /// 已连接设备缓存 (addr, name)
    pub devices: Vec<(String, String)>,
    /// 用户选择的推送目标设备地址
    pub selected_addr: Option<String>,
    /// 已成功注册 interconnect 接收的设备地址
    pub registered_devices: Vec<String>,
    /// 是否已启动设备列表定时刷新（避免重复挂定时器）
    pub device_timer_started: bool,
}

static STATE: OnceLock<Mutex<AppState>> = OnceLock::new();

pub fn state() -> &'static Mutex<AppState> {
    STATE.get_or_init(|| {
        Mutex::new(AppState {
            root_element_id: None,
            schedule_json: None,
            summary: None,
            file_name: None,
            import_status: "尚未导入课表".into(),
            push_status: String::new(),
            file_type: FileType::Json,
            devices: Vec::new(),
            selected_addr: None,
            registered_devices: Vec::new(),
            device_timer_started: false,
        })
    })
}
