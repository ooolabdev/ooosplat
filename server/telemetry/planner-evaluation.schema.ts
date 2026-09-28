import { z } from "zod";

const nullableCount = z.number().int().nonnegative().max(Number.MAX_SAFE_INTEGER).nullable();
const nullablePositiveNumber = z.number().finite().nonnegative().nullable();

export const plannerFramePlanSchema = z.object({
  sourceWidth: z.number().int().nonnegative(),
  sourceHeight: z.number().int().nonnegative(),
  sourceItemCount: z.number().int().nonnegative(),
  configuredTargetFps: nullablePositiveNumber,
  configuredCandidateFps: nullablePositiveNumber,
  effectiveTargetFps: nullablePositiveNumber,
  effectiveCandidateFps: nullablePositiveNumber,
  initialSelectedCount: z.number().int().nonnegative(),
  candidateCount: z.number().int().nonnegative(),
  minimumFrameOverrideApplied: z.boolean(),
  minimumFrameTargetUnreachable: z.boolean(),
}).strict();

export const plannerInitialReconstructionSchema = z.object({
  inputImages: z.number().int().nonnegative(),
  registeredImages: z.number().int().nonnegative(),
  points3d: z.number().int().nonnegative(),
  backend: z.enum(["cpu", "gpu"]),
  allowTwoViewTracks: z.boolean(),
}).strict();

export const plannerBridgeSchema = z.object({
  status: z.enum([
    "notApplicable",
    "notEvaluated",
    "notNeeded",
    "noBudget",
    "running",
    "completed",
    "failedRolledBack",
  ]),
  triggerRatio: z.number().finite().min(0).max(1),
  availableBudget: z.number().int().nonnegative(),
  requestedFrames: z.number().int().nonnegative(),
  addedFrames: z.number().int().nonnegative(),
  internalBridgeCount: z.number().int().nonnegative(),
  edgeExtensionCount: z.number().int().nonnegative(),
  durationMs: nullableCount,
  initialInputImages: z.number().int().nonnegative(),
  initialRegisteredImages: z.number().int().nonnegative(),
  initialPoints3d: z.number().int().nonnegative(),
  finalInputImages: z.number().int().nonnegative(),
  finalRegisteredImages: z.number().int().nonnegative(),
  finalPoints3d: z.number().int().nonnegative(),
  adopted: z.boolean(),
}).strict();

const brushProfileSchema = z.enum([
  "legacy",
  "fast",
  "balanced",
  "highLow",
  "highStandard",
  "highLarge",
  "highEmergency",
]);

export const plannerBrushSchema = z.object({
  initialProfile: brushProfileSchema.nullable(),
  finalProfile: brushProfileSchema.nullable(),
  detectedTotalMemoryMb: nullableCount,
  maxResolution: z.number().int().nonnegative().nullable(),
  totalSteps: nullableCount,
  growthGradThreshold: nullablePositiveNumber,
  growthSelectFraction: nullablePositiveNumber,
  growthStopIter: z.number().int().nonnegative().nullable(),
  refineEvery: z.number().int().nonnegative().nullable(),
  maxSplats: z.number().int().nonnegative().nullable(),
  oomRetryUsed: z.boolean(),
}).strict();

export const plannerFinalResultSchema = z.object({
  inputImages: z.number().int().nonnegative(),
  registeredImages: z.number().int().nonnegative(),
  points3d: z.number().int().nonnegative(),
  splatCount: z.number().int().nonnegative(),
  plySizeBytes: z.number().int().nonnegative(),
}).strict();

export const plannerStageDurationsSchema = z.object({
  probingVideoMs: nullableCount,
  extractingFramesMs: nullableCount,
  extractingFeaturesMs: nullableCount,
  matchingMs: nullableCount,
  reconstructingMs: nullableCount,
  validatingReconstructionMs: nullableCount,
  trainingSplatsMs: nullableCount,
  exportingMs: nullableCount,
}).strict();

export const plannerEvaluationPropertiesSchema = z.object({
  qualityPreset: z.enum(["fast", "balanced", "high"]),
  inputType: z.enum(["video", "images"]),
  plannerSchemaVersion: z.literal(1),
  runId: z.string().uuid(),
  runKind: z.enum(["new", "resume"]),
  outcome: z.enum(["completed", "failed"]),
  plannerEnabled: z.boolean(),
  plannerVersion: z.literal("quality_v2_planner_v1").nullable(),
  totalDurationMs: z.number().int().nonnegative(),
  failureStage: z.enum([
    "probing_video",
    "extracting_frames",
    "extracting_features",
    "matching",
    "reconstructing",
    "validating_reconstruction",
    "training_splats",
    "exporting",
    "unknown",
  ]).nullable(),
  errorCode: z.enum([
    "engine_unavailable",
    "invalid_input",
    "ffprobe_failed",
    "ffmpeg_failed",
    "colmap_feature_failed",
    "colmap_matching_failed",
    "colmap_mapper_failed",
    "low_registered_images",
    "brush_failed",
    "brush_out_of_memory",
    "brush_device_lost",
    "disk_space_low",
    "io_failed",
    "unknown",
  ]).nullable(),
  gpuVendor: z.enum(["nvidia", "amd", "intel", "apple", "other", "unknown"]),
  framePlan: plannerFramePlanSchema.nullable(),
  initialReconstruction: plannerInitialReconstructionSchema.nullable(),
  bridge: plannerBridgeSchema.nullable(),
  brush: plannerBrushSchema.nullable(),
  finalResult: plannerFinalResultSchema.nullable(),
  stageDurations: plannerStageDurationsSchema,
}).strict().superRefine((properties, context) => {
  if (properties.plannerEnabled !== (properties.plannerVersion !== null)) {
    context.addIssue({
      code: z.ZodIssueCode.custom,
      path: ["plannerVersion"],
      message: "plannerVersion must be set exactly when Planner is enabled",
    });
  }
  if (properties.outcome === "completed" &&
      (properties.failureStage !== null || properties.errorCode !== null)) {
    context.addIssue({
      code: z.ZodIssueCode.custom,
      path: ["outcome"],
      message: "completed evaluations cannot contain failure fields",
    });
  }
  if (properties.outcome === "failed" && properties.errorCode === null) {
    context.addIssue({
      code: z.ZodIssueCode.custom,
      path: ["errorCode"],
      message: "failed evaluations require a safe error code",
    });
  }
});

export const plannerEvaluationEnvelopeSchema = z.object({
  installId: z.string().uuid(),
  event: z.literal("planner_evaluation"),
  timestamp: z.string().datetime({ offset: true }),
  appVersion: z.string().min(1).max(64),
  os: z.enum(["windows", "macos", "linux", "unknown"]),
  arch: z.enum(["x86_64", "aarch64", "unknown"]),
  properties: plannerEvaluationPropertiesSchema,
}).strict();

export type PlannerEvaluationEnvelope = z.infer<typeof plannerEvaluationEnvelopeSchema>;

// Add plannerEvaluationEnvelopeSchema as one member of the existing
// z.discriminatedUnion("event", [...]) used by /api/telemetry/event.
