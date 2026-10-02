import { describe, expect, it } from "vitest";
import {
  EXPORT_ORBIT_DURATION_SECONDS,
  ORBIT_DEGREES_PER_SECOND,
  ORBIT_START_SECONDS,
  PREVIEW_ANIMATION_GLSL,
  PREVIEW_ANIMATION_WGSL,
  REVEAL_DURATION_SECONDS,
  SHOCKWAVE_DURATION_SECONDS,
  VIDEO_DURATION_SECONDS,
  animationPhaseAt,
  animationEffectsActive,
  orbitDegreesAt,
  robustEffectBounds,
  effectRadialLimitForBounds,
} from "./PreviewAnimation";

describe("PreviewAnimation", () => {
  it("uses identical full-model radial coverage in the app and offline viewer",()=>{
    const effect={center:[0,0,0] as [number,number,number],halfExtents:[1,1,1] as [number,number,number]};
    expect(effectRadialLimitForBounds(effect,effect)).toBe(1);
    expect(effectRadialLimitForBounds({...effect,halfExtents:[10,10,10]},effect)).toBeCloseTo(10);
  });
  it("uses the fixed 5 + 8 + 10 second export timeline", () => {
    expect(REVEAL_DURATION_SECONDS).toBe(5);
    expect(SHOCKWAVE_DURATION_SECONDS).toBe(8);
    expect(ORBIT_START_SECONDS).toBe(13);
    expect(EXPORT_ORBIT_DURATION_SECONDS).toBe(10);
    expect(VIDEO_DURATION_SECONDS).toBe(23);
  });

  it("switches phases exactly at the reveal and shockwave boundaries", () => {
    expect(animationPhaseAt(0)).toBe("reveal");
    expect(animationPhaseAt(4.999)).toBe("reveal");
    expect(animationPhaseAt(5)).toBe("shockwave");
    expect(animationPhaseAt(12.999)).toBe("shockwave");
    expect(animationPhaseAt(13)).toBe("orbit");
    expect(animationPhaseAt(23)).toBe("orbit");
    expect(animationEffectsActive(0)).toBe(true);
    expect(animationEffectsActive(12.999)).toBe(true);
    expect(animationEffectsActive(13)).toBe(false);
  });

  it("orbits at one revolution per 24 seconds throughout every phase", () => {
    expect(ORBIT_DEGREES_PER_SECOND).toBe(15);
    expect(orbitDegreesAt(5)).toBe(75);
    expect(orbitDegreesAt(13)).toBe(195);
    expect(orbitDegreesAt(23)).toBe(345);
  });

  it("uses the center 98 percent of splats for effect bounds", () => {
    const centers = new Float32Array(101 * 3);
    for (let index = 0; index < 100; index += 1) {
      centers[index * 3] = index;
      centers[index * 3 + 1] = index * 2;
      centers[index * 3 + 2] = index * 3;
    }
    centers[300] = 100_000;
    centers[301] = 200_000;
    centers[302] = 300_000;
    const bounds = robustEffectBounds(centers, { center: [0, 0, 0], halfExtents: [1, 1, 1] });
    expect(bounds.center).toEqual([50, 100, 150]);
    expect(bounds.halfExtents).toEqual([49, 98, 147]);
  });

  it("keeps reveal opacity deterministic while particle centers drift", () => {
    const colorModifier = PREVIEW_ANIMATION_GLSL.split("void modifySplatColor")[1];
    expect(colorModifier).not.toContain("ooosplatParticleHash(center");
    expect(colorModifier).toContain("float originalAlpha = color.a");
    expect(colorModifier).toContain("float stagedAlpha = originalAlpha * ooosplatReveal(center)");
    expect(colorModifier).not.toContain("originalAlpha * 0.22");
  });

  it("uses the OOOSplat theme blue for the shockwave transition", () => {
    expect(PREVIEW_ANIMATION_GLSL).toContain("vec3(0.117647, 0.360784, 1.0)");
    expect(PREVIEW_ANIMATION_WGSL).toContain("vec3f(0.117647, 0.360784, 1.0)");
    expect(PREVIEW_ANIMATION_GLSL).not.toContain("vec3(1.0, 0.68, 0.24)");
    expect(PREVIEW_ANIMATION_WGSL).not.toContain("vec3f(1.0, 0.68, 0.24)");
  });

  it("provides equivalent work-buffer hooks for WebGL2 and WebGPU", () => {
    for (const hook of ["modifySplatCenter", "modifySplatRotationScale", "modifySplatColor"]) {
      expect(PREVIEW_ANIMATION_GLSL).toContain(hook);
      expect(PREVIEW_ANIMATION_WGSL).toContain(hook);
    }
    expect(PREVIEW_ANIMATION_WGSL).toContain("loadOoosplatDeleted()");
    expect(PREVIEW_ANIMATION_WGSL).toContain("loadOoosplatSelected()");
  });
});
