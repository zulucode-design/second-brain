mod ai;
mod backup;
mod commands;
mod history;
mod search;
mod state;
mod types;
mod vault;

use state::AppState;
#[allow(unused_imports)]
use tauri::{Emitter, Manager};

#[cfg(not(target_os = "android"))]
use tauri::{
    image::Image,
    menu::{MenuBuilder, MenuItemBuilder},
    tray::TrayIconBuilder,
};

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    rustls::crypto::ring::default_provider()
        .install_default()
        .expect("Failed to install rustls crypto provider");

    let config = commands::load_app_config();
    #[cfg(not(target_os = "android"))]
    let show_tray = config.show_tray_icon;
    #[cfg(not(target_os = "android"))]
    let close_to_tray = config.close_to_tray && show_tray;
    let app_state = AppState::new(config);

    let mut builder = tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_fs::init())
        .plugin(tauri_plugin_opener::init())
        .setup(move |app| {
            if cfg!(debug_assertions) {
                app.handle().plugin(
                    tauri_plugin_log::Builder::default()
                        .level(log::LevelFilter::Info)
                        .build(),
                )?;
            }

            // On Android, set config dir from Tauri's path resolver
            #[cfg(target_os = "android")]
            {
                if let Ok(config_dir) = app.path().config_dir() {
                    commands::set_android_config_dir(config_dir);
                } else if let Ok(data_dir) = app.path().data_dir() {
                    commands::set_android_config_dir(data_dir);
                }
            }

            #[cfg(not(target_os = "android"))]
            if show_tray {
                setup_tray(app)?;
            }

            // Check CLI args for a .md file path on initial launch
            #[cfg(not(target_os = "android"))]
            {
                let file_arg = std::env::args().skip(1).find(|a| {
                    let a = a.trim();
                    !a.starts_with('-') && a.ends_with(".md")
                });
                if let Some(path) = file_arg {
                    let resolved = if std::path::Path::new(&path).is_absolute() {
                        std::path::PathBuf::from(&path)
                    } else {
                        std::env::current_dir().unwrap_or_default().join(&path)
                    };
                    if let Some(resolved_str) = resolved.to_str() {
                        let app_state = app.state::<AppState>();
                        let _ = app_state.pending_open_file.lock().map(|mut p| {
                            *p = Some(resolved_str.to_string());
                        });
                    }
                }
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
            commands::set_line_height,
            commands::get_notebooks,
            commands::create_notebook,
            commands::rename_notebook,
            commands::delete_notebook,
            commands::move_notebook,
            commands::get_notes,
            commands::read_note,
            commands::save_note,
            commands::create_note,
            commands::create_daily_note,
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
            commands::reorder_quick_access,
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
            commands::get_pending_open_file,
        ]);

    #[cfg(not(target_os = "android"))]
    {
        builder = builder.plugin(tauri_plugin_single_instance::init(|app, args, cwd| {
            // Always show/focus the main window
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.show();
                let _ = window.unminimize();
                let _ = window.set_focus();
            }

            // Extract .md file path from args (args[0] is the binary)
            let file_path = args.iter().skip(1).find(|arg| {
                let a = arg.trim();
                !a.starts_with('-') && a.ends_with(".md")
            });

            if let Some(path) = file_path {
                let resolved = if std::path::Path::new(path.as_str()).is_absolute() {
                    std::path::PathBuf::from(path)
                } else {
                    std::path::Path::new(&cwd).join(path)
                };
                if let Some(resolved_str) = resolved.to_str() {
                    let _ = app.emit("open-file", resolved_str.to_string());
                }
            }
        }));

        builder = builder.plugin(tauri_plugin_updater::Builder::new().build());

        builder = builder.on_window_event(move |window, event| {
            match event {
                tauri::WindowEvent::CloseRequested { api, .. } => {
                    // Only hide to tray for the main window
                    if close_to_tray && window.label() == "main" {
                        api.prevent_close();
                        let _ = window.hide();
                    }
                }
                tauri::WindowEvent::Destroyed => {
                    // When main window is destroyed, close all note windows
                    if window.label() == "main" {
                        let app = window.app_handle();
                        for (label, win) in app.webview_windows() {
                            if label.starts_with("note-") {
                                let _ = win.close();
                            }
                        }
                    }
                }
                _ => {}
            }
        });
    }

    builder
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

#[cfg(not(target_os = "android"))]
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
