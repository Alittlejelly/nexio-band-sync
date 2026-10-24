# Nexio 课程表推送（AstroBox 插件）

把 **Nexio 课程表** 的 JSON 课表文件导入 [AstroBox](https://abox.run)，一键推送到
小米手环 / 手表端的「Nexio课程表」快应用（[MI Band Pro 工程](../MI%20Band%20Pro)，
包名 `com.haooz.chedule`）。手环端打开课程表应用时，插件也会自动响应它的拉取请求。

Rust 编写，支持导入 v4 整表 JSON 与 WakeUp 等应用导出的课表 ICS（极简解析：周循环 RRULE / BYDAY / UNTIL / COUNT）。基于 AstroBox 插件模板
[AstroBox-NG-Plugin-Template-Rust](https://github.com/AstralSightStudios/AstroBox-NG-Plugin-Template-Rust)，
编译为 `wasm32-wasip2` 组件（WIT 绑定来自 crates.io 的 [`astrobox-ng-wit`](https://crates.io/crates/astrobox-ng-wit) 0.3.1，
，V4 `psys-world-v4` / `api_level 4`）。

## 工作原理

```
用户 ──「导入课表 JSON」──> dialog::pick_file 选择文件
                              │ schedule.rs 校验 + 归一化（补 protocol/version/action/sentAt）
                              ▼
                        内存中保存整表 JSON
                              │
用户 ──「推送到手环」──────> device::get_connected_device_list
                              │ interconnect::send_qaic_message(addr, "com.haooz.chedule", json)
                              ▼
                        手环端快应用 sync.js 收到 v4 整表 → 渲染 + 写缓存

手环端打开应用 ── action=request ──> register_interconnect_recv 注册的
                                      on_event(InterconnectMessage) → 自动回推整表
```

通信协议与手环端 `src/common/sync.js` 的约定一致（v4 整表，唯一支持的形态），
字段结构与手机端 NexioSchedule APK 的 `WatchPayload.buildFullJson` 相同。

## 使用

1. 手环上已安装「Nexio课程表」rpk，AstroBox 已连接手环
2. 把本插件 `dist/Nexio课程表推送.abp` 通过 AstroBox 安装
3. 打开插件页面 → 在「课表文件」卡选择格式（课表 JSON / 课表 ICS）→ 点「选择文件」选文件（可用本仓库 `sample-schedule.json` 试）
4. 检查摘要（课程数 / 学期开始 / 周数 / 假期），点「推送到手环」
5. 手环上打开课程表应用查看；之后每次打开应用它都会自动向插件拉最新数据

> 导入的课表保存在插件内存中，AstroBox 重启后需重新导入。

## 课表 JSON 格式（v4 整表）

顶层字段：

| 字段 | 说明 |
| --- | --- |
| `protocol` | 固定 `nexio.schedule`（缺失时插件会补上，写了别的值会被拒绝） |
| `version` | 固定 `4`（插件统一写 4） |
| `action` | `replace` 整表覆盖（默认）或 `clear` 清空 |
| `schedule_name` | 课表名称（兼容 `scheduleName`） |
| `settings.class_start_time` | 开学日 `YYYY/MM/DD`，手环据此推算教学周，**必填** |
| `settings.total_weeks` / `current_week` | 总周数 / 推送时的教学周 |
| `settings.morning_sections` 等 | 上/下/晚节数 |
| `times` | 节次时间表 `morning/afternoon/evening → {节次: "HH:MM-HH:MM"}` |
| `courses` | 课程数组（缺失则拒绝） |
| `holidays` | 假期 / 调休数组，调休优先用 `followDate`（绝对日期）换课 |

课程字段：`dayOfWeek`（1=周一…7=周日）、`startSection`/`endSection`（全局节次号）、
`startWeek`/`endWeek`、`weekType`（0 全周 / 1 单周 / 2 双周）、`selectedWeeks`（非空时优先）、
`isCustomTime` + `customStartTime`/`customEndTime`、`location`、`teacher`。

## 构建

```bash
rustup target add wasm32-wasip2   # 需要 Rust + Python 3
python scripts/build_dist.py --release --package
```

产物在 `dist/`：`nexio_band_sync.wasm`、`manifest.json`、`icon.png`，以及可直接
安装的 `Nexio课程表推送.abp`。

测试（课表解析，宿主 target）：

```bash
cargo test --target x86_64-pc-windows-gnu
```

## 权限

`manifest.json` 声明：

- `device` — 获取已连接设备列表
- `interconnect` — 向手环快应用发送消息
- `register_interconnect_recv` — 接收手环发来的 `request` 消息

## 目录结构

```
Cargo.toml            # 依赖 astrobox-ng-wit 0.3（tracing-appender 锁 0.2.3，0.2.5 在 wasip2 上编译不过）
manifest.json         # 插件清单（api_level 4，entry = nexio_band_sync.wasm）
.cargo/config.toml    # 默认 target = wasm32-wasip2
scripts/build_dist.py # 模板自带的构建打包脚本
src/lib.rs            # 入口：lifecycle + event_v3（on_event / on_ui_event_v3 / on_ui_render）
src/ui.rs             # 插件页面 UI（ui_v3 Builder API）与按钮流程
src/push.rs           # 设备发现、接收注册、推送、响应手环 request
src/schedule.rs       # v4 整表 JSON 的校验 + 归一化（含单元测试）
src/state.rs          # 全局状态（已导入课表、UI 文案）
sample-schedule.json  # 示例课表，可直接导入测试
template/             # 上游模板克隆（仅参考，构建不需要）
```

## 备注（排错记录）

- **首次打包安装失败**：最初按 api_level 3（旧接口集 `psys-world-v3`）构建。AstroBox
  对 api_level 2/3 用旧 wasmtime、api_level 4 用新 wasmtime（双运行时）；当前 rustc
  （stable 1.99.1 / nightly 1.101）为 wasm32-wasip2 发出的 WASI 导入是
  `wasi:*@0.2.12`，只有新运行时（wasmtime 48，其 wasmtime-wasi 自带 0.2.12）能实例化，
  旧运行时给不了 → 安装失败。改用 astrobox-ng-wit 0.4 + api_level 4（与官方 V4 插件
  同构）后组件导入 `astrobox:psys-host-v4`、导出 `astrobox:psys-plugin-v4`，
  与参考插件完全一致。
- V4 接口差异：宿主函数全 async、参数收 `String`（非 `&str`）、错误为
  `result<_, string>`、`pick-file` 返回 `result<pick-result, string>`；插件侧 Guest
  方法直接 `async fn`，不再需要 `wit_future::new()` + `FutureReader` 样板。
- 排错工具：`wasm-tools component wit xxx.wasm` 可直接看组件导入/导出的 world 与
  wasi 版本，和已知能装的 .abp 对比即知差异。
- 手环端协议细节见 MI Band Pro 工程 `src/common/sync.js` 头部注释。
