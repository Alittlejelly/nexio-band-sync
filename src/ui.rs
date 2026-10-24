//! 插件页面 UI（psys-world-v4 的 ui Builder API）
//! 布局参照 Loop 课表导入器：圆角行卡片 + 可点击行选择，全部文字左对齐。
//! 不使用原生 Select（宿主渲染样式不可控），设备与格式都用自绘行选择。

use crate::state::FileType;
use crate::{ics, push, schedule, state};
use astrobox_ng_wit::astrobox::psys_host_v4::{self, ui};

pub const EVENT_IMPORT: &str = "nexio_import_json";
pub const EVENT_PUSH: &str = "nexio_push_band";
pub const EVENT_REFRESH_DEVICES: &str = "nexio_refresh_devices";
pub const EVENT_FMT_JSON: &str = "nexio_fmt_json";
pub const EVENT_FMT_ICS: &str = "nexio_fmt_ics";
pub const EVENT_DEVICE_PICK_PREFIX: &str = "nexio_device_pick_";

const BG: &str = "#191919";
const CARD: &str = "#262626";
const CARD_HI: &str = "#333333";
const BLUE: &str = "#0090FF";
const RED: &str = "#C14848";
const WHITE: &str = "#FFFFFF";
const MUTED: &str = "#9A9A9A";

/// 统一的 UI 事件入口（Click 等）
pub async fn handle_ui_event(event_id: &str, event: ui::Event, _payload: &str) {
    if !matches!(event, ui::Event::Click) {
        return;
    }
    if let Some(idx) = event_id.strip_prefix(EVENT_DEVICE_PICK_PREFIX) {
        if let Ok(i) = idx.parse::<usize>() {
            pick_device(i).await;
        }
        return;
    }
    match event_id {
        EVENT_IMPORT => import_flow().await,
        EVENT_PUSH => push_flow().await,
        EVENT_REFRESH_DEVICES => {
            set_push_status("");
            refresh_devices().await;
            push::register_recv_on_connected_devices().await;
        }
        EVENT_FMT_JSON => {
            state::state().lock().unwrap_or_else(|p| p.into_inner()).file_type = FileType::Json;
            rerender();
        }
        EVENT_FMT_ICS => {
            state::state().lock().unwrap_or_else(|p| p.into_inner()).file_type = FileType::Ics;
            rerender();
        }
        _ => {}
    }
}

async fn pick_device(i: usize) {
    {
        let mut st = state::state().lock().unwrap_or_else(|p| p.into_inner());
        if let Some((addr, _)) = st.devices.get(i).cloned() {
            st.selected_addr = Some(addr);
        }
    }
    rerender();
}

async fn import_flow() {
    set_import_status("正在打开文件选择器…");
    rerender();

    let extensions: Vec<String> = {
        let st = state::state().lock().unwrap_or_else(|p| p.into_inner());
        match st.file_type {
            FileType::Json => vec!["json"],
            FileType::Ics => vec!["ics", "ical", "icalendar"],
        }
        .into_iter()
        .map(str::to_string)
        .collect()
    };

    let picked = psys_host_v4::dialog::pick_file(
        psys_host_v4::dialog::PickConfig {
            read: true,
            copy_to: None,
        },
        psys_host_v4::dialog::FilterConfig {
            multiple: false,
            extensions,
            default_directory: String::new(),
            default_file_name: String::new(),
        },
    )
    .await;

    let pick = match picked {
        Ok(p) => p,
        Err(e) => {
            set_import_status(format!("未选择文件：{e}"));
            rerender();
            return;
        }
    };
    if pick.data.is_empty() {
        set_import_status("未选择文件或文件为空");
        rerender();
        return;
    }
    set_import_status(format!("正在解析 {}…", pick.name));
    rerender();

    let lower = pick.name.to_ascii_lowercase();
    let looks_ics =
        lower.ends_with(".ics") || lower.ends_with(".ical") || lower.ends_with(".icalendar");
    let result = if looks_ics {
        // ICS：先转成 v4 整表，再走统一的归一化流程
        match std::str::from_utf8(&pick.data)
            .map_err(|_| "ICS 文件不是 UTF-8 文本".to_string())
            .and_then(|text| ics::ics_to_payload(text.trim_start_matches('\u{feff}'), &pick.name))
        {
            Ok(payload) => schedule::import_str(&payload.to_string()),
            Err(e) => Err(e),
        }
    } else {
        schedule::import_json(&pick.data)
    };

    match result {
        Ok(imported) => {
            let mut st = state::state().lock().unwrap_or_else(|p| p.into_inner());
            st.schedule_json = Some(imported.json);
            st.summary = Some(imported.summary);
            st.file_name = Some(pick.name);
            st.import_status = "导入成功 ✓".into();
        }
        Err(e) => {
            let mut st = state::state().lock().unwrap_or_else(|p| p.into_inner());
            st.schedule_json = None;
            st.summary = None;
            st.file_name = None;
            st.import_status = format!("导入失败：{e}");
        }
    }
    tracing::info!("课表文件处理完成");
    rerender();
}

async fn push_flow() {
    let selected = {
        let st = state::state().lock().unwrap_or_else(|p| p.into_inner());
        st.selected_addr.clone().and_then(|addr| {
            st.devices
                .iter()
                .find(|(a, _)| a == &addr)
                .map(|(_, name)| (addr, name.clone()))
        })
    };
    let json = state::state()
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .schedule_json
        .clone();
    let Some(json) = json else {
        set_push_status("请先导入课表文件");
        rerender();
        return;
    };
    let Some((addr, name)) = selected else {
        set_push_status("未选择设备：请先连接手环并在「目标设备」中选择");
        rerender();
        return;
    };

    set_push_status(format!("正在推送到「{name}」…"));
    rerender();

    let msg = push::push_to_device(&addr, &name, &json).await;
    set_push_status(msg);
    push::register_recv_on_connected_devices().await;
    rerender();
}

/// 查询已连接设备，更新设备列表；若原选择已失效则自动选第一台
pub async fn refresh_devices() {
    let devices = push::connected_devices().await;
    {
        let mut st = state::state().lock().unwrap_or_else(|p| p.into_inner());
        st.devices = devices
            .iter()
            .map(|d| (d.addr.clone(), d.name.clone()))
            .collect();
        let still_selected = st
            .selected_addr
            .as_ref()
            .map(|a| st.devices.iter().any(|(addr, _)| addr == a))
            .unwrap_or(false);
        if !still_selected {
            st.selected_addr = st.devices.first().map(|(a, _)| a.clone());
        }
    }
    rerender();
}

fn set_import_status(s: impl Into<String>) {
    state::state().lock().unwrap_or_else(|p| p.into_inner()).import_status = s.into();
}

fn set_push_status(s: impl Into<String>) {
    state::state().lock().unwrap_or_else(|p| p.into_inner()).push_status = s.into();
}

fn text(content: &str, size: u32, color: &str) -> ui::Element {
    ui::Element::new(ui::ElementType::P, Some(content))
        .size(size)
        .text_color(color)
}

/// 圆角行卡片（略亮于背景，内容纵向排列，内部间距用 gap）
fn card() -> ui::Element {
    ui::Element::new(ui::ElementType::Div, None)
        .flex()
        .flex_direction(ui::FlexDirection::Column)
        .width_full()
        .bg(CARD)
        .radius(18)
        .padding(16)
        .gap(6)
}

/// 原生 Button（点击保证可用）；text-align 尝试左对齐，宿主不支持则居中
fn action_button(label: &str, event_id: &str, bg: &str) -> ui::Element {
    ui::Element::new(ui::ElementType::Button, Some(label))
        .on(ui::Event::Click, event_id)
        .width_full()
        .height(50)
        .bg(bg)
        .radius(18)
        .padding_left(20)
        .size(15)
        .text_color(WHITE)
        .prop("text-align", "left")
}

pub fn build_main_ui() -> ui::Element {
    let st = state::state().lock().unwrap_or_else(|p| p.into_inner());

    let mut root = ui::Element::new(ui::ElementType::Div, None)
        .flex()
        .flex_direction(ui::FlexDirection::Column)
        .width_full()
        .height_full()
        .bg(BG)
        .padding(16);

    // ── 目标设备卡：可点击设备行 + 右上角刷新（绝对定位，不依赖 flex_grow）──
    let device_subtitle = if st.devices.is_empty() {
        "未发现设备，请先在 AstroBox 中连接手环"
    } else {
        "点击选择课表要推送到的设备"
    };
    let mut device_card = card()
        .relative()
        .child(text("目标设备", 16, WHITE))
        .child(text(device_subtitle, 12, MUTED))
        .child(
            ui::Element::new(ui::ElementType::Button, Some("刷新设备"))
                .on(ui::Event::Click, EVENT_REFRESH_DEVICES)
                .absolute()
                .top(14)
                .right(14)
                .z_index(1)
                .width(96)
                .height(32)
                .bg(BLUE)
                .radius(16)
                .size(13)
                .text_color(WHITE),
        );

    if st.devices.is_empty() {
        device_card = device_card.child(
            ui::Element::new(ui::ElementType::Div, None)
                .width_full()
                .bg(BG)
                .radius(12)
                .padding(12)
                .margin_top(4)
                .child(text("未连接设备", 14, RED)),
        );
    } else {
        for (i, (_, name)) in st.devices.iter().enumerate() {
            let is_selected = st
                .selected_addr
                .as_ref()
                .map(|a| st.devices.iter().position(|(addr, _)| addr == a) == Some(i))
                .unwrap_or(false);
            let (row_bg, name_color, mark) = if is_selected {
                (CARD_HI, WHITE, " ✓")
            } else {
                (BG, MUTED, "")
            };
            device_card = device_card.child(
                ui::Element::new(ui::ElementType::Div, None)
                    .flex()
                    .flex_direction(ui::FlexDirection::Row)
                    .align_center()
                    .width_full()
                    .bg(row_bg)
                    .radius(12)
                    .padding(12)
                    .margin_top(4)
                    .on(ui::Event::Click, &format!("{EVENT_DEVICE_PICK_PREFIX}{i}"))
                    .child(text(&format!("{name}{mark}"), 14, name_color)),
            );
        }
    }
    root = root.child(device_card.margin_bottom(12));

    // ── 课表文件卡：格式切换（自绘分段按钮）+ 导入状态 ──
    let fmt_pill = |label: &str, active: bool, event_id: &str| {
        let (bg, fg) = if active { (BLUE, WHITE) } else { (BG, MUTED) };
        ui::Element::new(ui::ElementType::Button, Some(label))
            .on(ui::Event::Click, event_id)
            .width_full()
            .height(36)
            .bg(bg)
            .radius(10)
            .size(13)
            .text_color(fg)
    };
    let mut file_card = card()
        .child(text("课表文件", 16, WHITE))
        .child(
            ui::Element::new(ui::ElementType::Div, None)
                .flex()
                .flex_direction(ui::FlexDirection::Row)
                .align_center()
                .width_full()
                .margin_top(4)
                .child(
                    // 各占一半、内侧留 4px 内边距 → 按钮间 8px 空隙，总宽不溢出
                    ui::Element::new(ui::ElementType::Div, None)
                        .flex()
                        .flex_direction(ui::FlexDirection::Row)
                        .width_half()
                        .padding_right(4)
                        .child(fmt_pill(
                            "课表 JSON",
                            st.file_type == FileType::Json,
                            EVENT_FMT_JSON,
                        )),
                )
                .child(
                    ui::Element::new(ui::ElementType::Div, None)
                        .flex()
                        .flex_direction(ui::FlexDirection::Row)
                        .width_half()
                        .padding_left(4)
                        .child(fmt_pill(
                            "课表 ICS",
                            st.file_type == FileType::Ics,
                            EVENT_FMT_ICS,
                        )),
                ),
        )
        .child(text(&st.import_status, 12, MUTED));
    if let Some(summary) = &st.summary {
        for line in summary.detail_lines() {
            file_card = file_card.child(text(&line, 12, MUTED));
        }
    }
    root = root.child(file_card.margin_bottom(12));

    // ── 操作按钮：选择文件（灰）+ 推送到手环（蓝）──
    root = root
        .child(action_button("选择文件", EVENT_IMPORT, CARD).margin_bottom(12))
        .child(action_button("推送到手环", EVENT_PUSH, BLUE));

    // ── 状态卡（有结果才显示）──
    if !st.push_status.is_empty() {
        root = root.child(
            card()
                .margin_top(12)
                .child(text("状态", 16, WHITE))
                .child(text(&st.push_status, 12, MUTED)),
        );
    }

    root = root
        .child(text("", 8, WHITE).margin_top(12))
        .child(text("支持 Nexio 课程表 v4 整表 JSON，或 WakeUp 等应用导出的课表 ICS。", 12, MUTED))
        .child(text("推送后请打开手环上的课程表应用查看；应用打开时也会自动向本插件拉取。", 12, MUTED));

    root
}

pub fn render_main_ui(element_id: &str) {
    {
        let mut st = state::state().lock().unwrap_or_else(|p| p.into_inner());
        st.root_element_id = Some(element_id.to_string());
    }
    // 打开页面时刷新设备列表，并为已连接设备补注册接收；
    // 另挂一个 15s 定时器周期刷新（手环晚于页面连接也能出现在列表里）
    astrobox_ng_wit::spawn(async {
        refresh_devices().await;
        push::register_recv_on_connected_devices().await;
        let first = {
            let mut st = state::state().lock().unwrap_or_else(|p| p.into_inner());
            let first = !st.device_timer_started;
            st.device_timer_started = true;
            first
        };
        if first {
            psys_host_v4::timer::set_interval(15000, "device_refresh");
        }
    });
    psys_host_v4::ui::render(element_id, build_main_ui());
}

pub fn rerender() {
    let root = state::state()
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .root_element_id
        .clone();
    if let Some(id) = root {
        psys_host_v4::ui::render(&id, build_main_ui());
    }
}
