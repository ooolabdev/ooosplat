import { readFileSync } from "node:fs";
import { expect, it } from "vitest";

it('mutes the entire saved material card like the readonly project and quality fields without changing editable cards', () => {
  const css = readFileSync(new URL('./styles.css', import.meta.url), 'utf8');
  const rules = [...css.matchAll(/([^{}]+)\{([^{}]*)\}/g)];
  const declarations = (selector: string) => rules.filter(([, selectors]) => selectors.split(',').some(value => value.trim() === selector))
    .map(([, , value]) => value).join(' ');
  expect(declarations('.material-inputs.read-only')).toContain('border-color: var(--line)');
  for (const selector of ['.material-inputs.read-only', '.material-inputs.read-only .material-input-summary',
    '.material-inputs.read-only .material-row', '.material-inputs.read-only .material-inspector']) {
    expect(declarations(selector)).toContain('background: #f2f4f7');
  }
  for (const selector of ['.material-input-summary strong', '.material-input-summary small', '.material-row-copy > strong',
    '.material-row-copy > small', '.material-row-main > svg', '.material-row-meta strong', '.material-row-type',
    '.material-members', '.colmap-path strong', '.colmap-path small', '.material-imported strong',
    '.material-imported small', '.colmap-inspector > .field-note']) {
    expect(declarations(`.material-inputs.read-only ${selector}`)).toContain('color: #7f8999');
  }
  expect(declarations('.material-input-summary strong')).toContain('color: var(--ink)');
  expect(declarations('.material-row-copy > strong')).toContain('color: var(--ink)');
  expect(declarations('.material-row-main > svg')).toContain('color: var(--blue)');
  expect(declarations('button:focus-visible')).toContain('outline: 2px solid');
});

it('removes the persistent frames from add-input buttons and the sampling source selector but keeps keyboard focus', () => {
  const css = readFileSync(new URL('./styles.css', import.meta.url), 'utf8');
  for (const selector of ['\\.material-add-actions button', '\\.material-empty button', '\\.frame-sampling-head select']) {
    const rule = css.match(new RegExp(`${selector}\\s*\\{[^}]+\\}`))?.[0];
    expect(rule).toContain('border: 1px solid transparent');
    expect(rule).toContain('background: transparent');
  }
  for (const selector of ['\\.material-add-actions button:hover:not\\(:disabled\\)', '\\.frame-sampling-head select:hover:not\\(:disabled\\)']) {
    expect(css.match(new RegExp(`${selector}\\s*\\{[^}]+\\}`))?.[0]).not.toMatch(/border|background/);
  }
  expect(css.match(/\.frame-sampling-head select:focus-visible\s*\{[^}]+\}/)?.[0]).toContain('outline: 2px solid');
  expect(css.match(/\.frame-sampling-head select\s*\{[^}]+\}/)?.[0]).toContain('width: 100%');
  expect(css.match(/\.frame-sampling-head output\s*\{[^}]+\}/)?.[0]).toContain('white-space: nowrap');
  expect(css.match(/button:focus-visible\s*\{[^}]+\}/)?.[0]).toContain('outline: 2px solid');
  expect(css.match(/\.colmap-discovery > \.secondary-action\s*\{[^}]+\}/)?.[0]).toContain('border: 1px solid var(--line-dark)');
});

it('sizes the sampling source picker from the current label while bounding it to the right column', () => {
  const css = readFileSync(new URL('./styles.css', import.meta.url), 'utf8');
  const picker = css.match(/\.frame-sampling-material-picker\s*\{[^}]+\}/)?.[0];
  const measure = css.match(/\.frame-sampling-material-measure\s*\{[^}]+\}/)?.[0];
  const select = css.match(/\.frame-sampling-head select\s*\{[^}]+\}/)?.[0];
  expect(picker).toContain('width: max-content');
  expect(picker).toContain('max-width: min(100%,210px)');
  expect(measure).toContain('visibility: hidden');
  expect(select).toContain('position: absolute');
  expect(select).toContain('font: inherit');
  expect(select).toContain('appearance: none');
  expect(css.match(/\.frame-sampling-material-arrow\s*\{[^}]+\}/)?.[0]).toContain('pointer-events: none');
  for (const rule of [measure, select]) expect(rule).toContain('padding: 2px 20px 2px 8px');
});

it('lets every metadata tag share the same gray backdrop as editable and readonly material details', () => {
  const css = readFileSync(new URL('./styles.css', import.meta.url), 'utf8');
  const tag = css.match(/\.source-attribute-tag\s*\{[^}]+\}/)?.[0];
  expect(tag).toContain('background: transparent');
  expect(tag).not.toContain('background: var(--paper)');
  expect(css.match(/\.material-inspector\s*\{[^}]+\}/)?.[0]).toContain('background: #f2f4f7');
  expect(css.match(/\.material-inputs\.read-only \.material-inspector\s*\{[^}]+\}/)?.[0]).toContain('background: #f2f4f7');
});

it('keeps frame sampling on the same background as the quality and retry controls', () => {
  const css = readFileSync(new URL('./styles.css', import.meta.url), 'utf8');
  expect(css.match(/\.frame-sampling-control\s*\{[^}]+\}/)?.[0]).toContain('background: inherit');
  expect(css.match(/\.frame-sampling-control\.disabled\s*\{[^}]+\}/)?.[0]).toBeUndefined();
  expect(css.match(/\.locked-quality-settings\s*\{[^}]+\}/)?.[0]).toContain('background: #f2f4f7');
  for (const selector of ['quality-option', 'planner-switch']) {
    expect(css.match(new RegExp(`\\.locked-quality-settings \\.${selector}\\s*\\{[^}]+\\}`))?.[0]).toContain('background: #f2f4f7');
  }
});

it('keeps Alpha in material details, gray when readonly, without obsolete standalone metrics styling', () => {
  const css = readFileSync(new URL('./styles.css', import.meta.url), 'utf8');
  expect(css).not.toContain('.source-metrics');
  expect(css.match(/\.alpha-source-status\.readonly\s*\{[^}]+\}/)?.[0]).toContain('color: #7f8999');
  expect(css.match(/\.source-attribute-tag\s*\{[^}]+\}/)?.[0]).not.toContain('border-bottom');
  expect(css.match(/\.alpha-source-status\s*\{[^}]+\}/)?.[0]).toBeUndefined();
  expect(css).toContain('.process-metrics small');
  expect(css).toContain('.process-metrics b');
});

it('keeps the media type layout and places only compact metadata tags on its right without wrapping', () => {
  const css = readFileSync(new URL('./styles.css', import.meta.url), 'utf8');
  const shared = css.match(/\.source-status-hint\s*\{[^}]+\}/)?.[0];
  expect(shared).toContain('display: flex');
  expect(shared).toContain('align-items: center');
  expect(shared).toContain('gap: 8px');
  expect(shared).toContain('color: #536078');
  expect(css.match(/\.source-status-hint strong\s*\{[^}]+\}/)?.[0]).toContain('font-weight: 600');
  expect(shared).not.toMatch(/border|background/);
  const tags = css.match(/\.source-attribute-tags\s*\{[^}]+\}/)?.[0];
  expect(tags).toContain('display: flex');
  expect(tags).toContain('flex-wrap: nowrap');
  expect(tags).toContain('margin-left: auto');
  expect(tags).toContain('overflow-x: auto');
  const tag = css.match(/\.source-attribute-tag\s*\{[^}]+\}/)?.[0];
  expect(tag).toContain('display: inline-flex');
  expect(tag).toContain('white-space: nowrap');
  expect(tag).toContain('color: #7183a0');
  expect(tag).toContain('border: 1px solid #dfe4ed');
  expect(css.match(/\.gps-source-hint\s*\{[^}]+\}/)?.[0]).toBeUndefined();
  expect(css.match(/\.material-row\.ready::after,[^}]+\}/)?.[0]).toContain('top: calc(50% - 3px)');
});

it("centers sampling independently of its captions and keeps preview speed outside the scrollable statistics", () => {
  const css = readFileSync(new URL("./styles.css", import.meta.url), "utf8");
  expect(css.match(/\.frame-sampling-head\s*\{[^}]+\}/)?.[0]).toContain("grid-template-columns: minmax(0,1fr) auto minmax(0,1fr)");
  expect(css.match(/\.frame-sampling-foot\s*\{[^}]+\}/)?.[0]).toContain("grid-template-columns: minmax(0,1fr) auto minmax(0,1fr)");
  expect(css.match(/\.preview-status-items\s*\{[^}]+\}/)?.[0]).toContain("overflow-x: auto");
  expect(css.match(/\.preview-roam-speed\s*\{[^}]+\}/)?.[0]).toContain("flex: 0 0 auto");
  expect(css.match(/\.preview-roam-speed input\s*\{[^}]+\}/)?.[0]).toContain("height: 18px");
  for (const pseudo of ['webkit-slider-runnable-track', 'moz-range-track']) {
    expect(css.match(new RegExp(`\\.preview-roam-speed input::-${pseudo}\\s*\\{[^}]+\\}`))?.[0]).toContain('height: 2px');
  }
});

it("keeps topbar controls frameless and changes only color on hover, preserving keyboard focus", () => {
  const css = readFileSync(new URL("./styles.css", import.meta.url), "utf8");
  const rule = (selector: string) => css.match(new RegExp(`${selector}\\s*\\{([^}]+)\\}`))?.[1];
  expect(rule("\\.settings-action")).toContain("background: transparent");
  expect(rule("\\.settings-action")).toContain("border: 1px solid transparent");
  for (const selector of ["\\.settings-action:hover", "\\.update-notice-action", "\\.update-notice-action:hover"]) {
    const declarations = rule(selector);
    expect(declarations).toContain("color:");
    expect(declarations).not.toMatch(/background|border|box-shadow/);
  }
  expect(rule("button:focus-visible")).toContain("outline: 2px solid");
});

it("rounds only standalone preview action buttons and gives update text an intrinsic-width column", () => {
  const css = readFileSync(new URL("./styles.css", import.meta.url), "utf8");
  expect(css).toContain('.preview-header-actions > button, .preview-export-menu > button:not(.export-menu-dismiss) { border-radius: 5px; }');
  expect(css.match(/\.preview-header-actions\s*\{[^}]+\}/)?.[0]).toContain("gap: 6px");
  expect(css.match(/\.update-settings-row\s*\{[^}]+\}/)?.[0]).toContain("minmax(0, 1fr) max-content");
  expect(css.match(/\.update-check-button\s*\{[^}]+\}/)?.[0]).toContain("white-space: nowrap");
});

it("keeps subdued new-task details distinct from readonly captions without changing media-row controls", () => {
  const css = readFileSync(new URL("./styles.css", import.meta.url), "utf8");
  const readonlyRule = css.match(/\.source-input-card\.readonly \.media-source-copy > strong,[^}]+\}/)?.[0];
  expect(readonlyRule).toContain(".source-input-card.readonly .media-source-copy > p");
  expect(readonlyRule).toContain(".source-input-card.readonly .media-source-type-icon");
  expect(readonlyRule).toContain("color: #7f8999");
  expect(css.match(/\.media-source-type-icon\s*\{[^}]+\}/)?.[0]).toContain("color: #7183a0");
  expect(css.match(/\.material-row-main > svg\s*\{[^}]+\}/)?.[0]).toContain("color: var(--blue)");
  expect(css.match(/\.source-status-hint\s*\{[^}]+\}/)?.[0]).toContain("align-items: center");
  expect(css.match(/\.path-picker\.media-path\s*\{[^}]+\}/)?.[0]).toContain("grid-template-columns: minmax(0, 1fr)");
});
