//! 设备发现、手环消息接收注册与课表推送

use crate::schedule::PROTOCOL;
use crate::state;
use astrobox_ng_wit::astrobox::psys_host_v4;
use serde_json::Value;

/// 手环端课程表快应用的包名（两端 interconnect 对端必须一致）
pub const BAND_PKG: &str = "com.haooz.chedule";

pub async fn connected_devices() -> Vec<psys_host_v4::device::DeviceInfo> {
    psys_host_v4::device::get_connected_device_list().await
}

/// 为所有已连接设备注册 interconnect 接收，
/// 这样手环端课程表应用打开后发来的 action=request 会被推给本插件。
pub async fn register_recv_on_connected_devices() {
    for d in connected_devices().await {
        let already = {
            let mut st = state::state().lock().unwrap_or_else(|p| p.into_inner());
            if st.registered_devices.iter().any(|a| a == &d.addr) {
                true
            } else {
                st.registered_devices.push(d.addr.clone());
                false
            }
        };
        if already {
            continue;
        }
        match psys_host_v4::register::register_interconnect_recv(d.addr.clone(), BAND_PKG.to_string()).await {
            Ok(()) => tracing::info!("已注册手环消息接收：{} ({})", d.name, d.addr),
            Err(e) => {
                tracing::warn!("注册手环消息接收失败：{} ({}): {}", d.name, d.addr, e);
                // 回滚登记，下次（页面渲染 / 推送时）重试
                state::state()
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .registered_devices
                    .retain(|a| a != &d.addr);
            }
        }
    }
}

/// 把整表 JSON 推给所有已连接设备，返回给用户看的结果文案
pub async fn push_to_all(json: &str) -> String {
    let devices = connected_devices().await;
    if devices.is_empty() {
        return "未找到已连接的手环，请先在 AstroBox 中连接设备".into();
    }
    let mut ok = 0usize;
    let mut lines: Vec<String> = Vec::new();
    for d in &devices {
        match psys_host_v4::interconnect::send_qaic_message(d.addr.clone(), BAND_PKG.to_string(), json.to_string()).await {
            Ok(()) => {
                ok += 1;
                lines.push(format!("「{}」成功", d.name));
            }
            Err(e) => lines.push(format!("「{}」失败：{}", d.name, e)),
        }
    }
    format!("已推送到 {ok}/{} 台设备：{}", devices.len(), lines.join("；"))
}

/// 推送到指定设备（用户在下拉框选择的推送目标）
pub async fn push_to_device(addr: &str, name: &str, json: &str) -> String {
    match psys_host_v4::interconnect::send_qaic_message(
        addr.to_string(),
        BAND_PKG.to_string(),
        json.to_string(),
    )
    .await
    {
        Ok(()) => format!("已推送到「{name}」✓"),
        Err(e) => format!("推送到「{name}」失败：{e}"),
    }
}

/// 手环消息入口：识别 action=request 并自动回推整表
pub async fn on_band_message(payload: &str) {
    let Ok(v) = serde_json::from_str::<Value>(payload) else {
        tracing::info!("收到无法解析的手环消息：{payload}");
        return;
    };
    // 部分实现会把消息包一层 { data: "..." }
    let v = if let Some(s) = v.get("data").and_then(Value::as_str).map(str::to_owned) {
        serde_json::from_str(&s).unwrap_or(v)
    } else {
        v
    };

    let protocol_ok = v
        .get("protocol")
        .and_then(Value::as_str)
        .map(|p| p == PROTOCOL)
        .unwrap_or(false);
    let is_request = v.get("action").and_then(Value::as_str) == Some("request");
    if !protocol_ok || !is_request {
        tracing::info!("忽略手环消息（非课表 request）");
        return;
    }

    tracing::info!("手环请求课表，自动推送");
    let json = state::state()
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .schedule_json
        .clone();
    if let Some(json) = json {
        let msg = push_to_all(&json).await;
        {
            let mut st = state::state().lock().unwrap_or_else(|p| p.into_inner());
            st.push_status = format!("手环请求课表 → {msg}");
        }
        crate::ui::rerender();
    }
}
