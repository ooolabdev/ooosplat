export const EDITABLE_SPLAT_ASSET_OPTIONS = Object.freeze({
  data: Object.freeze({ reorder: false }),
});

export function requiredSplatTextureSide(splatCount: number) {
  return Math.ceil(Math.sqrt(Math.max(0, splatCount)));
}

export function splatTextureCapacityError(splatCount: number, maximumTextureSide: number, locale: Locale = "zh-CN") {
  const requiredTextureSide = requiredSplatTextureSide(splatCount);
  return requiredTextureSide > maximumTextureSide
    ? translate(locale, "viewer.textureCapacity", { maximum: maximumTextureSide, required: requiredTextureSide })
    : null;
}
import { translate, type Locale } from "../../i18n";
