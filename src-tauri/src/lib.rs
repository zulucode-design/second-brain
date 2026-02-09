mod ai;
mod backup;
mod commands;
mod history;
mod search;
mod state;
mod types;
mod vault;

use state::AppState;
use tauri::{
    image::Image,
    menu::{MenuBuilder, MenuItemBuilder},
    tray::TrayIconBuilder,
    Manager,
};

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let config = commands::load_app_config();
    let show_tray = config.show_tray_icon;
    let close_to_tray = config.close_to_tray && show_tray;
    let app_state = AppState::new(config);

    let mut builder = tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_fs::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .setup(move |app| {
            if cfg!(debug_assertions) {
                app.handle().plugin(
                    tauri_plugin_log::Builder::default()
                        .level(log::LevelFilter::Info)
                        .build(),
                )?;
            }

            if show_tray {
                setup_tray(app)?;
            }

            Ok(())
        })
        .manage(app_state)
        .invoke_handler(tauri::generate_handler![
            commands::open_vault,
            commands::get_app_config,
            commands::set_theme,
            commands::set_accent_color,
            commands::set_font_size,
            commands::set_font_family,
            commands::get_notebooks,
            commands::create_notebook,
            commands::rename_notebook,
            commands::delete_notebook,
            commands::get_notes,
            commands::read_note,
            commands::save_note,
            commands::create_note,
            commands::rename_note,
            commands::delete_note,
            commands::move_note,
            commands::get_all_tags,
            commands::get_all_note_titles,
            commands::search_notes,
            commands::reindex,
            commands::get_trash,
            commands::restore_note,
            commands::permanent_delete,
            commands::empty_trash,
            commands::load_vault_state,
            commands::save_vault_state,
            commands::save_image,
            commands::save_attachment,
            commands::get_notebook_icons,
            commands::set_notebook_icon,
            commands::set_general_settings,
            commands::get_quick_access,
            commands::add_quick_access,
            commands::remove_quick_access,
            commands::get_vault_stats,
            commands::import_obsidian,
            commands::open_file,
            commands::copy_file_to,
            commands::create_backup,
            commands::list_backups,
            commands::restore_backup,
            commands::delete_backup,
            commands::set_backup_settings,
            commands::get_note_versions,
            commands::get_note_version_content,
            commands::create_version,
            commands::set_ai_settings,
            commands::test_ai_connection,
            commands::ai_ask,
            commands::get_install_type,
        ]);

    if close_to_tray {
        builder = builder.on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
            }
        });
    }

    builder
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

fn setup_tray(app: &mut tauri::App) -> Result<(), Box<dyn std::error::Error>> {
    let show = MenuItemBuilder::with_id("show", "Show HelixNotes").build(app)?;
    let quit = MenuItemBuilder::with_id("quit", "Quit").build(app)?;
    let menu = MenuBuilder::new(app).items(&[&show, &quit]).build()?;

    let _tray = TrayIconBuilder::new()
        .icon(
            Image::from_path("icons/32x32.png")
                .unwrap_or_else(|_| app.default_window_icon().cloned().unwrap()),
        )
        .menu(&menu)
        .tooltip("HelixNotes")
        .on_menu_event(|app, event| match event.id().as_ref() {
            "show" => {
                if let Some(window) = app.get_webview_window("main") {
                    let _ = window.show();
                    let _ = window.unminimize();
                    let _ = window.set_focus();
                }
            }
            "quit" => {
                app.exit(0);
            }
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let tauri::tray::TrayIconEvent::Click { .. } = event {
                if let Some(window) = tray.app_handle().get_webview_window("main") {
                    let _ = window.show();
                    let _ = window.unminimize();
                    let _ = window.set_focus();
                }
            }
        })
        .build(app)?;

    Ok(())
}
