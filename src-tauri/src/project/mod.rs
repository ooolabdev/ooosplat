pub mod catalog;
pub mod creation;
pub mod manager;
pub mod metadata;

pub use manager::{ProjectImportObserver, ProjectImportProgress, ProjectManager, ProjectPaths};
pub use metadata::{
    ExternalFileFingerprint, FrameState, GaussianCrop, GaussianEditing, GaussianTransform,
    ImageMatchingStrategy, ImportedColmapProvenance, ImportedGaussianFormat,
    ImportedProjectProvenance, PipelineStateFile, ProjectInputType, ProjectMetadata, ProjectOutput,
    ProjectStatus, ReshootProvenance, ReshootState, COLMAP_INPUT_VERSION, PROJECT_APP_ID,
};
