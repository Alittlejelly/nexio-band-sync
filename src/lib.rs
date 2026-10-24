use astrobox_ng_wit::exports::astrobox::psys_plugin_v4::{
    event::{self, EventType},
    lifecycle,
};

mod ics;
mod logger;
mod push;
mod schedule;
mod state;
mod ui;

struct Plugin;

impl event::Guest for Plugin {
    async fn on_event(event_type: EventType, event_payload: String) -> String {
        match event_type {
            EventType::InterconnectMessage => {
                push::on_band_message(&event_payload).await;
            }
            EventType::Timer => {
                if event_payload == "device_refresh" {
                    ui::refresh_devices().await;
                    push::register_recv_on_connected_devices().await;
                }
            }
            _ => {}
        }
        String::new()
    }

    async fn on_ui_event(event_id: String, event: event::UiEvent, event_payload: String) -> String {
        ui::handle_ui_event(&event_id, event, &event_payload).await;
        String::new()
    }

    async fn on_ui_render(element_id: String) {
        ui::render_main_ui(&element_id);
    }

    async fn on_card_render(_card_id: String) {}
}

impl lifecycle::Guest for Plugin {
    async fn on_load() {
        logger::init();
        tracing::info!("Nexio 课程表推送插件已加载");
        // 尽早为已连接的设备注册 interconnect 接收；
        // 之后连接的设备会在页面打开 / 推送时补注册。
        push::register_recv_on_connected_devices().await;
    }
}

astrobox_ng_wit::export!(Plugin);
