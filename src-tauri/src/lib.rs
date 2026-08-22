pub mod commands;
pub mod engines;
pub mod error;
pub mod pipeline;
pub mod presets;
pub mod process;
pub mod project;
pub mod reconstruction;
pub mod video;

pub fn run_app() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .manage(commands::PipelineController::default())
        .invoke_handler(tauri::generate_handler![
            commands::check_engines,
            commands::probe_and_plan,
            commands::start_pipeline,
            commands::cancel_pipeline,
            commands::export_ply,
            commands::get_project_overview,
            commands::set_projects_root,
            commands::set_colmap_acceleration,
            commands::delete_project,
        ])
        .run(tauri::generate_context!())
        .expect("failed to run OOOSplat");
}
