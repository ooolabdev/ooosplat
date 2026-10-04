// @vitest-environment jsdom

import { beforeEach, describe, expect, it } from "vitest";
import { loadUiPreferences, saveUiPreferences, UI_PREFERENCES_STORAGE_KEY } from "./uiPreferences";

describe("UI preferences", () => {
  beforeEach(() => localStorage.clear());

  it("hides the runtime panel by default and for damaged storage", () => {
    expect(loadUiPreferences()).toEqual({ showRuntimePanel: false });
    localStorage.setItem(UI_PREFERENCES_STORAGE_KEY, "{");
    expect(loadUiPreferences()).toEqual({ showRuntimePanel: false });
  });

  it("persists the runtime panel choice across reloads", () => {
    saveUiPreferences({ showRuntimePanel: true });
    expect(loadUiPreferences()).toEqual({ showRuntimePanel: true });
  });

  it("ignores unknown or invalid fields", () => {
    localStorage.setItem(UI_PREFERENCES_STORAGE_KEY, JSON.stringify({ showRuntimePanel: "yes", future: true }));
    expect(loadUiPreferences()).toEqual({ showRuntimePanel: false });
  });
});
