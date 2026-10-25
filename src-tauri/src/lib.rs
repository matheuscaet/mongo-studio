mod assistant;
mod commands;
mod connection;
mod connections_io;
mod driver;
mod ejson;
mod error;
mod export;
mod logging;
mod mcp_server;
mod models;
mod mongosh;
mod paths;
mod relations;
mod saved_scripts;
mod scripting;
mod secrets;
mod sidebar_layout;
mod ssh_tunnel;
mod state;

use tauri::Manager;

use assistant::Assistant;
use mcp_server::McpServer;
use mongosh::Mongosh;
use saved_scripts::SavedScriptsStore;
use sidebar_layout::SidebarLayoutStore;
use state::AppState;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // First, while this is still the only thread.
    logging::init_local_offset();
    logging::install_panic_hook();

    tauri::Builder::default()
        // First, so what the other plugins log is caught too.
        .plugin(logging::plugin())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            logging::log_startup(app);
            setup(app).inspect_err(|e| log::error!("startup failed: {e}"))
        })
        .invoke_handler(tauri::generate_handler![
            commands::list_connection_profiles,
            commands::get_connection_profile,
            commands::save_connection_profile,
            commands::set_legacy_uuid_encoding,
            commands::delete_connection_profile,
            commands::test_connection,
            commands::connect,
            commands::disconnect,
            commands::secret_backend_info,
            commands::list_databases,
            commands::list_collections,
            commands::get_collection_stats,
            commands::run_find,
            commands::run_aggregate,
            commands::update_field,
            commands::count_documents,
            commands::run_script,
            commands::cancel_script,
            commands::cancel_query,
            commands::export_query,
            commands::export_value,
            commands::cancel_export,
            commands::list_index_stats,
            commands::explain_query,
            commands::export_connections,
            commands::import_connections,
            commands::preview_connections_import,
            commands::get_sidebar_layout,
            commands::save_sidebar_layout,
            commands::suggest_script_path,
            commands::save_script,
            commands::list_saved_scripts,
            commands::read_saved_script,
            commands::assistant_detect,
            commands::assistant_set_policy,
            commands::assistant_start,
            commands::assistant_send,
            commands::assistant_stop,
            commands::assistant_close,
            commands::assistant_answer,
            commands::assistant_workdir,
            commands::open_log_dir,
            commands::mongosh_detect,
            commands::mongosh_download,
            commands::mcp_set_config,
            commands::mcp_status,
            commands::mcp_token,
            commands::mcp_regenerate_token,
            commands::mcp_answer,
            commands::mcp_disconnect_client,
            commands::mcp_client_targets,
            commands::mcp_register_client,
            commands::mcp_unregister_client,
            commands::relations_list,
            commands::relation_save,
            commands::relation_delete,
            commands::relation_target_connection,
            commands::relation_find_related,
            commands::relation_suggest_links,
            commands::relation_check,
        ])
        .build(tauri::generate_context!())
        .expect("error while running tauri application")
        .run(|_, event| {
            // Tells a clean quit apart from a crash, which ends the log
            // without this line.
            if let tauri::RunEvent::Exit = event {
                log::info!("Mongo Studio exiting");
            }
        });
}

fn setup(app: &mut tauri::App) -> Result<(), Box<dyn std::error::Error>> {
    let config_dir = app.path().app_config_dir()?;
    let state =
        AppState::init(&config_dir).map_err(|e| -> Box<dyn std::error::Error> { e.into() })?;
    app.manage(state);
    let scripts = SavedScriptsStore::load(&config_dir, &app.path().home_dir()?)
        .map_err(|e| -> Box<dyn std::error::Error> { e.to_string().into() })?;
    app.manage(scripts);
    let layout = SidebarLayoutStore::load(&config_dir)
        .map_err(|e| -> Box<dyn std::error::Error> { e.to_string().into() })?;
    app.manage(layout);
    let relations = relations::RelationsStore::load(&config_dir)
        .map_err(|e| -> Box<dyn std::error::Error> { e.to_string().into() })?;
    app.manage(relations);
    // The agents run in an empty folder of the app's own, so they see none
    // of the user's files - one they can see at all, see `paths`.
    let workdir = paths::shared_dir(app.path().app_data_dir()?.join("assistant"), "assistant");
    app.manage(Assistant::new(
        std::sync::Arc::new(app.handle().clone()),
        workdir,
    ));
    app.manage(Mongosh::new(app.path().app_data_dir()?.join("mongosh")));
    app.manage(McpServer::new(app.handle().clone()));
    Ok(())
}
