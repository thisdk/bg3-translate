//! BG3 MOD 汉化工具 —— Tauri 桌面壳入口。
//!
//! 这里只负责组装：插件、状态、命令注册、退出清理。
//! 所有业务逻辑在 `bg3-translate-core`。

mod commands;
mod state;

use bg3_translate_core::pak;
use state::AppState;
use tauri::{Manager, RunEvent};

/// 注册给前端的命令清单。
macro_rules! handlers {
    () => {
        tauri::generate_handler![
            // 归档
            commands::open_mod,
            commands::extract_mod,
            commands::repack_mod,
            commands::close_mod,
            commands::work_dir_alive,
            // 条目
            commands::read_file_entries,
            commands::write_file_entries,
            // 翻译
            commands::translate_entries,
            commands::cancel_translation,
            // 设置
            commands::save_llm_settings,
            commands::load_llm_settings,
            commands::app_info,
            // 术语表
            commands::list_glossary,
            commands::add_glossary_entry,
            commands::update_glossary_entry,
            commands::delete_glossary_entry,
            commands::reset_glossary,
            commands::import_glossary,
        ]
    };
}

/// 构建日志插件：控制台 + 日志目录双写。
fn log_plugin() -> tauri::plugin::TauriPlugin<tauri::Wry> {
    use tauri_plugin_log::{Target, TargetKind};

    let level = if cfg!(debug_assertions) {
        log::LevelFilter::Debug
    } else {
        log::LevelFilter::Info
    };

    tauri_plugin_log::Builder::new()
        .level(level)
        .targets([
            Target::new(TargetKind::Stdout),
            Target::new(TargetKind::LogDir {
                file_name: Some("bg3-translate".into()),
            }),
        ])
        .build()
}

/// 启动应用。
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let app = tauri::Builder::default()
        .plugin(log_plugin())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_fs::init())
        .manage(AppState::default())
        .invoke_handler(handlers!())
        .build(tauri::generate_context!())
        .expect("构建 Tauri 应用失败");

    app.run(|handle, event| {
        if let RunEvent::ExitRequested { .. } | RunEvent::Exit = event {
            // 退出时清理临时工作目录，避免 %TEMP% 里堆一堆解包结果
            let state = handle.state::<AppState>();
            if let Some(dir) = state.take_work_dir() {
                log::info!("退出：清理工作目录 {}", dir.display());
                pak::remove_work_dir(&dir);
            }
        }
    });
}
