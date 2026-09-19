pub mod catalog;
pub mod manager;
pub mod metadata;

pub use manager::{ProjectManager, ProjectPaths};
pub use metadata::{
    FrameState, GaussianCrop, GaussianEditing, GaussianTransform, PipelineStateFile,
    ProjectInputType, ProjectMetadata, ProjectOutput, ProjectStatus, ReshootProvenance,
    PROJECT_APP_ID,
};
