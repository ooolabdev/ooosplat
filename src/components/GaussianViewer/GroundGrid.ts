import {
  BLEND_NONE,
  BoundingBox,
  Color,
  GraphNode,
  LAYERID_WORLD,
  Mesh,
  MeshInstance,
  PRIMITIVE_LINES,
  StandardMaterial,
  Vec3,
  type Application,
  type Layer,
} from "playcanvas";

export interface GroundGridGeometry {
  positions: Vec3[];
  colors: Color[];
  step: number;
  halfSize: number;
  bounds: BoundingBox;
}

export function niceGridStep(span: number) {
  const raw = Math.max(span, 0.0001) / 12;
  const exponent = Math.floor(Math.log10(raw));
  const magnitude = 10 ** exponent;
  const normalized = raw / magnitude;
  const factor = normalized <= 1 ? 1 : normalized <= 2 ? 2 : normalized <= 5 ? 5 : 10;
  return factor * magnitude;
}

export function buildGroundGrid(modelBounds: BoundingBox): GroundGridGeometry {
  const span = Math.max(modelBounds.halfExtents.x * 2, modelBounds.halfExtents.y * 2, modelBounds.halfExtents.z * 2, 0.0001);
  const step = niceGridStep(span);
  const halfSize = step * 10;
  const positions: Vec3[] = [];
  const colors: Color[] = [];
  const minor = new Color(0.18, 0.21, 0.27);
  const major = new Color(0.28, 0.32, 0.4);
  const xAxis = new Color(0.9, 0.25, 0.22);
  const yAxis = new Color(0.22, 0.74, 0.39);
  const zAxis = new Color(0.2, 0.43, 0.96);

  const addLine = (start: Vec3, end: Vec3, color: Color) => {
    positions.push(start, end);
    colors.push(color, color);
  };

  for (let index = -10; index <= 10; index += 1) {
    if (index === 0) continue;
    const offset = index * step;
    const color = index % 5 === 0 ? major : minor;
    addLine(new Vec3(-halfSize, 0, offset), new Vec3(halfSize, 0, offset), color);
    addLine(new Vec3(offset, 0, -halfSize), new Vec3(offset, 0, halfSize), color);
  }

  addLine(new Vec3(-halfSize, 0, 0), new Vec3(halfSize, 0, 0), xAxis);
  addLine(new Vec3(0, 0, -halfSize), new Vec3(0, 0, halfSize), zAxis);
  addLine(new Vec3(0, 0, 0), new Vec3(0, step * 2, 0), yAxis);

  const bounds = new BoundingBox(new Vec3(0, step, 0), new Vec3(halfSize, step, halfSize));
  return { positions, colors, step, halfSize, bounds };
}

export class GroundGrid {
  private destroyed = false;
  private readonly worldLayer: Layer;
  private readonly meshInstance: MeshInstance;
  private readonly material: StandardMaterial;
  private readonly node: GraphNode;
  readonly bounds: BoundingBox;

  constructor(app: Application, bounds: BoundingBox) {
    const geometry = buildGroundGrid(bounds);
    this.bounds = geometry.bounds.clone();
    const worldLayer = app.scene.layers.getLayerById(LAYERID_WORLD);
    if (!worldLayer) throw new Error("PlayCanvas World layer is unavailable");
    this.worldLayer = worldLayer;

    const mesh = new Mesh(app.graphicsDevice);
    mesh.setPositions(geometry.positions.flatMap(({ x, y, z }) => [x, y, z]));
    mesh.setColors(geometry.colors.flatMap(({ r, g, b, a }) => [r, g, b, a]));
    mesh.update(PRIMITIVE_LINES);

    const material = new StandardMaterial();
    material.name = "OOOSplat Ground Grid";
    material.useLighting = false;
    material.useTonemap = false;
    material.useFog = false;
    material.diffuse.set(0, 0, 0);
    material.emissive.set(1, 1, 1);
    material.emissiveVertexColor = true;
    material.blendType = BLEND_NONE;
    material.depthTest = true;
    material.depthWrite = true;
    material.update();
    this.material = material;

    const node = new GraphNode("OOOSplat Ground Grid");
    this.node = node;
    const meshInstance = new MeshInstance(mesh, material, node);
    meshInstance.castShadow = false;
    this.meshInstance = meshInstance;
    worldLayer.addMeshInstances([meshInstance], true);
  }

  setVisible(visible: boolean) {
    this.meshInstance.visible = visible;
  }

  get isVisible() {
    return this.meshInstance.visible;
  }

  destroy() {
    if (this.destroyed) return;
    this.destroyed = true;
    this.worldLayer.removeMeshInstances([this.meshInstance], true);
    this.meshInstance.destroy();
    this.material.destroy();
    this.node.destroy();
  }
}
