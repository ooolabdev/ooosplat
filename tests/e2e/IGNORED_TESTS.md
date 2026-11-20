# Existing explicitly ignored Rust tests

The default all-target suites retain these pre-existing ignores (48 library + 1 CLI). No `--ignored` engine/GPU/user-data acceptance was enabled. The SQLite crash child is launched explicitly by its existing parent regression; its ignore prevents standalone execution. E2E CLI compiles the same CLI test module and therefore lists the same one ignored test again.

| Test | Existing reason |
| --- | --- |
| `commands::html_export::tests::offline_viewer_browser_fixture` | Build the offline runtime first, then generate a browser verification fixture |
| `commands::model_export::tests::large_standard_ply_export_smoke` | Manual 1.099 GiB/5M Gaussian export smoke; needs 2.4 GB temporary disk space |
| `commands::model_export::tests::large_virtual_view_browser_smoke` | Real browser encoding above the former 512 MiB estimate; uses an isolated 400K SH3 fixture |
| `commands::model_export::tests::virtual_view_browser_smoke` | Manual browser + Rust edited-view smoke; set OOOSPLAT_EXPORT_SMOKE_INPUT to a small real PLY |
| `commands::tests::fast_folder_detection_cache_benchmark` | Header-only preflight cold/cache-hit timing, isolated PNG fixtures |
| `commands::tests::fast_video_detection_benchmark_and_single_probe_cache` | Requires bundled FFprobe and a real ordinary/panorama video path |
| `commands::tests::real_panorama_metadata_and_saved_detail_without_preview_extraction` | Requires OOOSPLAT_PANORAMA_DETAIL_TEST_PROJECT and the bundled engines; read-only real input verification |
| `engines::colmap::initialization_runtime_tests::native_071_matching_and_mapper_keep_premultiplied_rgb_and_grayscale_masks` | Requires explicit prepared RGB/grayscale-Mask native regression manifest |
| `engines::colmap::initialization_runtime_tests::native_invalid_initialization_recovers_without_reextracting_or_changing_masks` | Requires explicit read-only native regression manifest and bundled COLMAP |
| `engines::colmap::mapper_native_smoke::native_mapper_geometry_rejection_keeps_verified_baseline` | Explicit pinned native COLMAP smoke; synthetic geometry, CPU only, isolated new directory |
| `engines::colmap::tests::bundled_offline_loop_matching_smoke` | Requires the locked bundled COLMAP runtime; runs only CPU extraction and matching |
| `engines::colmap::tests::native_erp_features_without_masks_and_skipped_images_postcheck` | Bundled CPU ERP extraction; synthetic or OOOSPLAT_FEATURE_REGRESSION_IMAGES read-only input, no matching, Mapper or Brush |
| `engines::colmap::tests::native_reshoot_features_confirm_reused_and_independent_camera_ids` | Requires the local real PNG and bundled COLMAP; feature extraction only in an isolated database |
| `engines::colmap::tests::native_reshoot_retrieval_queries_only_registered_old_images` | Requires the local Ballintoy DB and bundled vocabulary; read-only retrieval in an isolated copied database |
| `engines::colmap_ba::tests::native_mixed_camera_fixed_constraints_probe` | Native Caspar mixed-camera/fixed-pose probe; writes a new isolated .cache fixture only |
| `engines::ffmpeg::tests::duplicate_pts_selected_frames_are_not_dropped` | requires bundled FFmpeg |
| `engines::ffmpeg::tests::selected_and_bridge_alpha_integration` | requires bundled FFmpeg and an external alpha video |
| `engines::ffprobe::tests::native_probe_large_metadata_and_limit` | requires local bundled FFmpeg/FFprobe; creates only isolated tiny synthetic videos |
| `engines::health::tests::managed_ooobrush_cli_health_smoke` | Requires npm run setup:brush on a matching native host |
| `pipeline::runner::gps_runner::tests::native_public_gps_generation` | Requires pinned native engines, the BSD-2-Clause Brighton Beach photos and a training GPU; writes an isolated .cache project |
| `pipeline::runner::gps_runner::tests::native_public_gps_matching_and_cache` | Requires completed public photo smoke; real native horizontal-only matching and cache, no training |
| `pipeline::runner::input_dataset::tests::native_compatibility_undistortion_preserves_alpha_and_source_files` | Bundled COLMAP compatibility conversion of two synthetic images; no features, matching, Mapper or Brush training |
| `pipeline::runner::input_dataset::tests::native_imported_public_photos_train_without_sfm` | Read-only public-photo import + bundled undistorter/Brush, 8 smoke iterations in an isolated directory; not quality acceptance |
| `pipeline::runner::input_dataset::tests::native_toy_mask_training_preparation_benchmark` | Read-only 251-frame toyWithMask preparation benchmark; isolated derived files, no SfM or Brush training |
| `pipeline::runner::inputs_runner::gps_resume_validation::real_gps_project_continues_to_training_without_rerunning_engines` | Set OOOSPLAT_VALIDATION_RESUME_PROJECT to a GPS project; only an isolated workspace copy is mutated |
| `pipeline::runner::inputs_runner::tests::native_joint_erp_and_ordinary_reconstruction_from_real_pixels` | Bundled COLMAP + real Ballintoy pixels with derived front views; joint SfM smoke only, cancelled before training |
| `pipeline::runner::recovery_8016f801::finalize_8016f801_failed_state_without_starting_engines` | operator-only finalization of the validated and installed incident recovery |
| `pipeline::runner::recovery_8016f801::recover_8016f801_without_starting_engines` | operator-only recovery; reads frozen evidence and writes only the explicitly selected target |
| `pipeline::runner::repeated_reshoot_validation::actual_completed_reshoot_v2_can_be_used_as_a_reshoot_source` | Set OOOSPLAT_REPEATED_RESHOOT_PROJECT to a registered completed Reshoot V2 project; source is read-only |
| `pipeline::runner::reshoot_runner::tests::native_ballintoy_reshoot_normal_parameters` | Requires real Ballintoy media, bundled engines and GPU; runs the normal full training recipe in an isolated .cache project |
| `pipeline::runner::reshoot_runner::tests::native_ballintoy_reshoot_v2` | Requires the local real Ballintoy source project, bundled engines, and GPU; writes only an isolated .cache project |
| `pipeline::runner::reshoot_runner::tests::native_public_gps_reshoot` | Requires the completed public GPS smoke project and native engines/GPU; repeated photographs test integration, not new detail |
| `pipeline::runner::tests::bundled_panorama_cancel_and_resume` | Requires OOOSPLAT_PANORAMA_TEST_PROJECT and the bundled GPU runtime |
| `pipeline::runner::tests::bundled_panorama_images_full_pipeline` | Requires real ERP photos and bundled engines; isolated cache only |
| `pipeline::runner::tests::bundled_panorama_progress_full_pipeline` | Requires OOOSPLAT_PANORAMA_VIDEO and bundled COLMAP/Brush; full real pipeline in isolated cache |
| `pipeline::runner::tests::native_legacy_shared_frames_create_a_current_derived_snapshot_without_source_changes` | Set OOOSPLAT_LEGACY_RESHOOT_PROJECT to a completed pre-colmap_frames project; source is read-only |
| `pipeline::runner::tests::native_manual_bridge_v3_extraction_and_rollback` | Runs bundled FFmpeg/FFprobe on isolated synthetic video; no SfM or training |
| `pipeline::runner::tests::replay_real_matching_log_keeps_index_counts_and_unknown_stage_percentage` | Requires OOOSPLAT_MATCHING_LOG, an existing real COLMAP matching log |
| `pipeline::splat_budget::tests::real_history_budget_matches_the_previously_calculated_ceiling` | read-only check against the user's existing 90-view native-resolution project |
| `pipeline::sqlite_recovery::tests::crashed_writer_child` | isolated SQLite crash fixture, invoked by its parent test |
| `reconstruction::reference_view::tests::actual_training_reference_view` | Requires OOOSPLAT_REFERENCE_PROJECT containing a real Brush training dataset |
| `tasks::logs::real_read_regressions::real_completed_task_log_pages_and_historical_readers_are_read_only` | requires OOOSPLAT_ACTIVITY_VALIDATION_REGISTRY naming an existing local task registry |
| `tasks::tests::real_activity_journal_count_redaction_read_only_regression` | requires OOOSPLAT_ACTIVITY_VALIDATION_REGISTRY naming an existing local task registry |
| `video::image_projection::tests::header_detection_benchmark` | Timing evidence with local real source; creates only isolated test header fixtures |
| `video::image_sequence::tests::real_ordinary_sfm_derivatives_match_historical_success_pixels` | requires explicit source/reference folders; reads two real photos and writes only its temporary derivatives |
| `video::panorama::tests::native_ordinary_rotated_mov_is_not_a_panorama_error` | Read-only IMG_8023.MOV regression; requires the local phone video and bundled FFprobe |
| `video::panorama::tests::native_resolution_density_diagnostic_cache` | Requires an existing real panorama image project; writes isolated cache only |
| `video::temporary_png_focal_workaround::tests::bundled_png_focal_regression` | Read-only original JPG/PNG + bundled COLMAP; writes new isolated diagnostic files only |
| `input_admission_tests::real_original_ordinary_alpha_fast_validation_continue` | Requires the original 90-photo Fast validation project, pinned native engines and GPU; continues only its isolated task |
