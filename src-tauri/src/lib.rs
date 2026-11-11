#![recursion_limit = "256"]

pub mod commands;
pub mod diagnostics;
pub mod engines;
pub mod error;
pub mod failure;
pub mod imports;
pub mod mcp;
pub mod pipeline;
pub mod planner;
pub mod presets;
pub mod process;
pub mod project;
pub mod reconstruction;
pub mod tasks;
pub mod telemetry;
pub mod updates;
pub mod video;

#[cfg(all(test, windows, target_env = "msvc"))]
mod windows_manifest_tests {
    #[test]
    fn test_executable_embeds_common_controls_v6_without_a_sidecar() {
        use windows_sys::Win32::System::LibraryLoader::{
            FindResourceW, GetModuleHandleW, LoadResource, LockResource, SizeofResource,
        };
        // Read the executing EXE's embedded RT_MANIFEST (24), resource ID 1.
        // This cannot accidentally pass by finding an external .exe.manifest.
        unsafe {
            let module = GetModuleHandleW(std::ptr::null());
            assert!(!module.is_null());
            let resource = FindResourceW(
                module,
                std::ptr::without_provenance(1),
                std::ptr::without_provenance(24),
            );
            assert!(
                !resource.is_null(),
                "The test EXE must embed its own manifest"
            );
            let size = SizeofResource(module, resource) as usize;
            let loaded = LoadResource(module, resource);
            let data = LockResource(loaded);
            assert!(size > 0 && !data.is_null());
            let xml =
                std::str::from_utf8(std::slice::from_raw_parts(data.cast::<u8>(), size)).unwrap();
            assert!(xml.contains("Microsoft.Windows.Common-Controls"));
            assert!(xml.contains("6.0.0.0"));
            assert!(xml.contains("6595b64144ccf1df"));
        }
    }
}

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
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(tauri_plugin_opener::init())
        .manage(commands::PipelineController::default())
        .manage(commands::PreviewController::default())
        .manage(mcp::exports::ExportController::default())
        .manage(telemetry::TelemetryService::new())
        .manage(updates::UpdateService::default())
        .manage(imports::ImportController::default())
        .on_page_load(|webview, payload| {
            use tauri::Manager;
            if webview.label() == "main"
                && matches!(payload.event(), tauri::webview::PageLoadEvent::Started)
            {
                if let Some(controller) = webview.try_state::<mcp::exports::ExportController>() {
                    controller.page_loading(webview.app_handle().clone());
                }
            }
        })
        .setup(|app| {
            use tauri::Manager;
            let tasks = app.state::<commands::PipelineController>().inner().clone();
            tasks.attach(app.handle().clone());
            tasks.spawn_notification_pump();
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
            commands::classify_material_paths,
            commands::classify_generation_paths,
            commands::inspect_material_inputs,
            commands::estimate_material_inputs,
            commands::detect_material_gps,
            commands::start_gui_inputs,
            updates::check_app_update,
            commands::check_engines,
            commands::check_colmap_acceleration,
            commands::probe_and_plan,
            commands::analyze_input,
            commands::detect_image_gps,
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
            mcp::exports::mcp_export_executor_ready,
            mcp::exports::mcp_export_executor_stopped,
            mcp::exports::update_mcp_model_export,
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
            imports::select_gaussian_import_source,
            imports::probe_gaussian_import,
            imports::begin_gaussian_import,
            imports::read_gaussian_import_chunk,
            imports::write_gaussian_import_chunk,
            imports::complete_gaussian_import,
            imports::cancel_gaussian_import,
            imports::relink_imported_project,
            commands::set_projects_root,
            commands::set_planner_enabled,
            commands::delete_project,
            commands::prepare_gaussian_preview,
            commands::read_gaussian_preview_chunk,
            commands::release_gaussian_preview,
            commands::save_gaussian_transform,
            commands::begin_gaussian_edit_save,
            commands::commit_gaussian_edit_save,
            commands::reset_gaussian_edits,
            commands::export_transformed_gaussian,
            commands::model_export::begin_gaussian_model_export,
            commands::model_export::prepare_gaussian_model_export,
            commands::model_export::start_gaussian_model_encoding,
            commands::model_export::read_gaussian_model_export_chunk,
            commands::model_export::write_gaussian_model_export_chunk,
            commands::model_export::complete_gaussian_model_export,
            commands::model_export::cancel_gaussian_model_export,
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
