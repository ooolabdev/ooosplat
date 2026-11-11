import type { FramePlan, ImageSequenceInfo, ImageMatchingStrategy, ProjectionDetection, ReshootCameraRelation, VideoInfo, VideoProjection } from './pipeline';
export type InputSource = {
    kind: 'video' | 'imageFolder';
    path: string;
} | {
    kind: 'images';
    paths: string[];
};
export interface MaterialInput {
    id: string;
    source: InputSource;
    identity?: string | null;
    projection: VideoProjection;
    imageMatchingStrategy: ImageMatchingStrategy;
    cameraRelation?: ReshootCameraRelation;
    confirmedSmallImages?: boolean;
    confirmedImageOrder?: boolean;
    /** Version 5+ video sampling: frames removed per second. Null uses the quality default. */
    frameRemovalFps?: number | null;
}
export interface ColmapInput {
    model: string;
    images: string;
    database?: string | null;
    identity?: string | null;
}
export interface ColmapModelCandidate {
    model: string;
    imageCandidates: string[];
}
export interface ColmapDiscovery {
    models: ColmapModelCandidate[];
    database?: string | null;
    selectedModel?: string | null;
}
export type GenerationPathSelection = {
    kind: 'materials';
    items: MaterialInput[];
} | {
    kind: 'colmap';
    input: ColmapInput;
} | {
    kind: 'colmapDiscovery';
    discovery: ColmapDiscovery;
};
export type GenerationInputs = {
    mode: 'materials';
    items: MaterialInput[];
} | {
    mode: 'colmap';
    input: ColmapInput;
};
export interface FrameSamplingRange {
    minimumTargetFps: number;
    maximumTargetFps: number;
    minimum: FramePlan;
    maximum: FramePlan;
}
export interface MaterialAnalysis {
    id: string;
    identity: string;
    legacyIdentity?:string|null;
    video: VideoInfo | null;
    imageSequence: ImageSequenceInfo | null;
    dimensions?: [
        number,
        number
    ][];
    projectionDetection: ProjectionDetection;
    plan: FramePlan | null;
    frameSamplingRange?: FrameSamplingRange | null;
    gpsDetected?: boolean;
    gpsFallback?: boolean;
}
/** Local summary only. Coordinates and geographic origins never enter the UI. */
export interface GpsAssistanceSummary {
    detectedSourceIds: string[];
    status: 'detected' | 'enabled' | 'fallback' | 'unavailable';
}
export interface ColmapInspection {
    identity: string;
    imageCount: number;
    points: number;
    cameraCount: number;
    hasErp: boolean;
    reshootEligible: boolean;
}
export interface InputsInspection {
    materials: MaterialAnalysis[];
    colmap: ColmapInspection | null;
    errors: Record<string, string>;
}
export interface SavedInputs {
    gpsAssistance?: GpsAssistanceSummary | null;
    request: GenerationInputs;
    sources: Array<{
        id: string;
        identity: string;
        projection: VideoProjection;
        names: string[];
        video: VideoInfo | null;
        imageSequence?: ImageSequenceInfo | null;
        dimensions?: [
            number,
            number
        ][];
        frames?: {
            retentionRatio: number;
            samplingFps: number;
            estimatedFrames: number;
            initialExtractedFrames?: number;
            extractedFrames?: number | null;
            minimumFrameOverrideApplied?: boolean;
        } | null;
        adopted: number;
    }>;
    importedReconstruction: boolean;
    unused: string[];
    sourceAdoption?: Array<{
        sourceId: string;
        plannedFrames: number;
        preparedFrames: number;
        registeredFrames: number;
        adopted: boolean;
        omittedReason?: string | null;
    }>;
}
export const sourcePath = (source: InputSource) => source.kind === 'images' ? source.paths[0] ?? '' : source.path;
