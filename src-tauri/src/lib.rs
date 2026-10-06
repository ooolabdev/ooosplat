#![recursion_limit = "256"]

pub mod commands;
pub mod diagnostics;
pub mod engines;
pub mod error;
pub mod mcp;
pub mod pipeline;
pub mod planner;
pub mod presets;
pub mod process;
pub mod project;
pub mod reconstruction;
pub mod tasks;
pub mod telemetry;
pub mod video;

pub fn run_app() {
    tauri::Builder::default()
        // Keep this first so a second process exits before other plugins initialize.
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            use tauri::Manager;

            if let Some(window) = app.get_webview_window("main") {
                let _ = window.unminimize();
                let _ = window.show();
                let _ = window.set_focus();
            }
        }))
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .manage(commands::PipelineController::default())
        .manage(commands::PreviewController::default())
        .manage(telemetry::TelemetryService::new())
        .setup(|app| {
            use tauri::Manager;
            let tasks = app.state::<commands::PipelineController>().inner().clone();
            tasks.attach(app.handle().clone());
            tasks.spawn_checkpoint_writer();
            let mcp = app.state::<mcp::McpController>().inner().clone();
            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                if let Err(e) = tasks.initialize().await {
                    tracing::error!(code=%e.code,"Task registry initialization failed");
                }
                if let Err(e) = tasks.import_projects().await {
                    tracing::warn!(code=%e.code,"Some legacy projects could not be imported");
                }
                if let Ok(settings) = project::catalog::load_settings().await {
                    if settings.mcp.enabled {
                        let _ = mcp.configure(handle, settings.mcp).await;
                    }
                }
            });
            Ok(())
        })
        .manage(mcp::McpController::default())
        .invoke_handler(tauri::generate_handler![
            commands::check_engines,
            commands::check_colmap_acceleration,
            commands::probe_and_plan,
            commands::classify_dropped_input,
            commands::estimate_project_runtime,
            commands::start_pipeline,
            commands::create_gui_task,
            commands::start_gui_task,
            commands::get_shared_tasks,
            commands::get_shared_task,
            commands::read_shared_task_logs,
            commands::cancel_shared_task,
            commands::resume_gui_task,
            commands::start_gui_reshoot,
            mcp::get_mcp_settings,
            mcp::set_mcp_settings,
            commands::inspect_reshoot_source,
            commands::probe_reshoot_input,
            commands::start_incremental_reshoot_pipeline,
            commands::resume_pipeline,
            commands::cancel_pipeline,
            commands::get_app_runtime_status,
            commands::open_project_location,
            commands::export_ply,
            commands::get_project_overview,
            commands::get_project_task_detail,
            commands::set_projects_root,
            commands::set_planner_enabled,
            commands::delete_project,
            commands::prepare_gaussian_preview,
            commands::release_gaussian_preview,
            commands::save_gaussian_transform,
            commands::begin_gaussian_edit_save,
            commands::commit_gaussian_edit_save,
            commands::reset_gaussian_edits,
            commands::export_transformed_gaussian,
            commands::begin_gaussian_video_export,
            commands::commit_gaussian_video_export,
            commands::cancel_gaussian_video_export,
            commands::html_export::begin_gaussian_html_export,
            commands::html_export::commit_gaussian_html_export,
            commands::html_export::cancel_gaussian_html_export,
            commands::initialize_telemetry,
            commands::set_telemetry_consent,
            diagnostics::prepare_error_report,
            diagnostics::send_error_report,
        ])
        .run(tauri::generate_context!())
        .expect("failed to run OOOSplat");
}
