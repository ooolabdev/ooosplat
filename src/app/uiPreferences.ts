export const UI_PREFERENCES_STORAGE_KEY = "ooo-splat-ui-preferences-v1";

export interface UiPreferences {
  showRuntimePanel: boolean;
}

export const DEFAULT_UI_PREFERENCES: UiPreferences = { showRuntimePanel: false };

export function loadUiPreferences(): UiPreferences {
  try {
    const value = JSON.parse(window.localStorage.getItem(UI_PREFERENCES_STORAGE_KEY) ?? "null") as Partial<UiPreferences> | null;
    return {
      showRuntimePanel: typeof value?.showRuntimePanel === "boolean" ? value.showRuntimePanel : false,
    };
  } catch {
    return DEFAULT_UI_PREFERENCES;
  }
}

export function saveUiPreferences(preferences: UiPreferences): void {
  try {
    window.localStorage.setItem(UI_PREFERENCES_STORAGE_KEY, JSON.stringify(preferences));
  } catch { /* optional local UI preference */ }
}
