import { BLEND_NONE, BoundingBox, Mesh, StandardMaterial, Vec3, type Application } from "playcanvas";
import { describe, expect, it, vi } from "vitest";
import { buildGroundGrid, GroundGrid, niceGridStep } from "./GroundGrid";

describe("GroundGrid", () => {
  it("chooses readable 1/2/5 grid steps for different model scales", () => {
    expect(niceGridStep(0.12)).toBeCloseTo(0.01);
    expect(niceGridStep(12)).toBe(1);
    expect(niceGridStep(24)).toBe(2);
    expect(niceGridStep(60)).toBe(5);
  });

  it("builds an XZ grid with colored origin axes", () => {
    const geometry = buildGroundGrid(new BoundingBox(new Vec3(), new Vec3(6, 2, 3)));

    expect(geometry.step).toBe(1);
    expect(geometry.halfSize).toBe(10);
    expect(geometry.positions).toHaveLength(86);
    expect(geometry.colors).toHaveLength(geometry.positions.length);
    expect(geometry.bounds.halfExtents.x).toBe(geometry.halfSize);
    expect(geometry.bounds.halfExtents.z).toBe(geometry.halfSize);
    expect(geometry.positions.at(-2)?.equals(new Vec3(0, 0, 0))).toBe(true);
    expect(geometry.positions.at(-1)?.equals(new Vec3(0, 2, 0))).toBe(true);
    expect(geometry.colors.at(-1)?.g).toBeGreaterThan(geometry.colors.at(-1)?.r ?? 1);
  });

  it("renders as an opaque depth-writing mesh and releases it once", () => {
    const updateMesh = vi.spyOn(Mesh.prototype, "update").mockImplementation(() => {});
    const destroyMaterial = vi.spyOn(StandardMaterial.prototype, "destroy");
    const addMeshInstances = vi.fn();
    const removeMeshInstances = vi.fn();
    const worldLayer = { id: 0, addMeshInstances, removeMeshInstances };
    const app = {
      graphicsDevice: {},
      scene: { layers: { getLayerById: vi.fn(() => worldLayer) } },
    } as unknown as Application;
    const grid = new GroundGrid(app, new BoundingBox(new Vec3(), new Vec3(1, 1, 1)));

    expect(updateMesh).toHaveBeenCalledTimes(1);
    expect(addMeshInstances).toHaveBeenCalledWith([expect.any(Object)], true);
    const meshInstance = addMeshInstances.mock.calls[0][0][0];
    expect(meshInstance.node).not.toBeNull();
    expect(meshInstance.material.blendType).toBe(BLEND_NONE);
    expect(meshInstance.material.depthTest).toBe(true);
    expect(meshInstance.material.depthWrite).toBe(true);
    expect(meshInstance.material.useLighting).toBe(false);
    grid.setVisible(false);
    expect(grid.isVisible).toBe(false);
    expect(meshInstance.visible).toBe(false);
    grid.setVisible(true);
    expect(grid.isVisible).toBe(true);
    expect(meshInstance.visible).toBe(true);
    grid.destroy();
    grid.destroy();
    expect(removeMeshInstances).toHaveBeenCalledTimes(1);
    expect(removeMeshInstances).toHaveBeenCalledWith([meshInstance], true);
    expect(destroyMaterial).toHaveBeenCalledTimes(1);
  });
});
