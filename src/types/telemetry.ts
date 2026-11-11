export type TelemetryDeliveryStatus = "notConfigured" | "configured" | "debug";

export interface TelemetryPreferences {
  analyticsEnabled: boolean;
  consentDecided: boolean;
  deliveryStatus: TelemetryDeliveryStatus;
  settingsError?: string | null;
}
