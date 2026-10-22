//! 应用入口：外壳装配（托盘 / 单实例 / 自启 / 数据层 / IPC）
//!
//! M1 行为约定（内存策略见后端方案 §10.3）：
//!   - 主窗口"懒创建、关闭即销毁"：关闭窗口 = 完全释放界面内存，程序收进托盘待机
//!   - 重复双击 = 唤起已有窗口（单实例）；窗口若已关闭销毁，则按配置重建
//!   - 自启拉起时带 --minimized 参数 → 直接藏在托盘（连窗口都不创建）
//!   - 开发期（debug）不同步自启状态，避免把 debug exe 注册进开机项

mod commands;
mod data;
mod ocr;
mod parser;
mod reminder;
mod theme;

use tauri::{
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    AppHandle, Manager, RunEvent, WebviewWindowBuilder,
};
use tauri_plugin_autostart::MacosLauncher;

use crate::data::DataStore;

pub fn run() {
    tauri::Builder::default()
        // 单实例插件必须最先注册
        .plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            show_main_window(app);
        }))
        .plugin(tauri_plugin_autostart::init(
            MacosLauncher::LaunchAgent,
            Some(vec!["--minimized"]),
        ))
        .setup(|app| {
            // 数据层：程序目录\data（原子写 + 启动备份）
            let store = DataStore::init()?;
            app.manage(store);

            build_tray(app.handle())?;

            // M2 提醒引擎：AUMID 注册 → 补提醒 → 托盘标记 → 30s 心跳线程
            reminder::start(app.handle());

            // 常规启动：开主窗口；自启（--minimized）：留在托盘，窗口保持懒创建
            if !std::env::args().any(|a| a == "--minimized") {
                show_main_window(app.handle());
            }

            // 正式版：启动时按设置同步一次自启状态（开发期跳过）
            if !cfg!(debug_assertions) {
                let enabled = app.state::<DataStore>().with(|d| d.settings.auto_launch);
                use tauri_plugin_autostart::ManagerExt;
                let manager = app.autolaunch();
                let _ = if enabled {
                    manager.enable()
                } else {
                    manager.disable()
                };
            }

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::app_info,
            commands::events_list,
            commands::events_create,
            commands::events_update,
            commands::events_remove,
            commands::events_set_completed,
            commands::settings_get,
            commands::settings_update,
            commands::reminders_status,
            commands::parse_text,
            commands::theme_extract_colors,
            commands::theme_save,
            commands::theme_load,
            commands::ocr_extract_text,
            commands::ocr_extract_bytes,
            commands::ocr_clipboard_text,
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|_app, event| {
            // 关闭最后一个窗口 ≠ 退出：程序收进托盘继续待机（真退出走托盘菜单）
            if let RunEvent::ExitRequested { code, api, .. } = event {
                if code.is_none() {
                    api.prevent_exit();
                }
            }
        });
}

fn build_tray(app: &AppHandle) -> tauri::Result<()> {
    let open_item = MenuItem::with_id(app, "open", "打开主窗口", true, None::<&str>)?;
    let quit_item = MenuItem::with_id(app, "quit", "退出", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&open_item, &quit_item])?;

    TrayIconBuilder::with_id("main")
        .icon(app.default_window_icon().expect("内置图标缺失").clone())
        .tooltip("桌面日历")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "open" => show_main_window(app),
            "quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                show_main_window(tray.app_handle());
            }
        })
        .build(app)?;
    Ok(())
}

/// 唤起主窗口：已存在则显示前置；已关闭销毁则按 tauri.conf.json 配置重建（懒创建）
fn show_main_window(app: &AppHandle) {
    if let Some(win) = app.get_webview_window("main") {
        let _ = win.show();
        let _ = win.unminimize();
        let _ = win.set_focus();
        return;
    }

    let Some(conf) = app
        .config()
        .app
        .windows
        .iter()
        .find(|w| w.label == "main")
        .cloned()
    else {
        eprintln!("[window] tauri.conf.json 中找不到 main 窗口配置");
        return;
    };

    match WebviewWindowBuilder::from_config(app, &conf).and_then(|builder| builder.build()) {
        Ok(_) => {}
        Err(e) => eprintln!("[window] 重建主窗口失败: {e}"),
    }
}
