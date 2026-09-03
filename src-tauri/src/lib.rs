pub mod commands;
pub mod engines;
pub mod error;
pub mod pipeline;
pub mod presets;
pub mod process;
pub mod project;
pub mod reconstruction;
pub mod telemetry;
pub mod video;

pub fn run_app() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .manage(commands::PipelineController::default())
        .manage(commands::PreviewController::default())
        .manage(telemetry::TelemetryService::new())
        .invoke_handler(tauri::generate_handler![
            commands::check_engines,
            commands::check_colmap_acceleration,
            commands::probe_and_plan,
            commands::start_pipeline,
            commands::resume_pipeline,
            commands::cancel_pipeline,
            commands::export_ply,
            commands::get_project_overview,
            commands::set_projects_root,
            commands::delete_project,
            commands::prepare_gaussian_preview,
            commands::release_gaussian_preview,
            commands::save_gaussian_transform,
            commands::export_transformed_gaussian,
            commands::begin_gaussian_video_export,
            commands::commit_gaussian_video_export,
            commands::cancel_gaussian_video_export,
            commands::initialize_telemetry,
            commands::set_telemetry_consent,
        ])
        .run(tauri::generate_context!())
        .expect("failed to run OOOSplat");
}
