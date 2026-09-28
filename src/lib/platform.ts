// Supported OS identity is injected by the backend before app scripts run.
type SupportedPlatform = { linux: boolean; windows: boolean };
const injected: SupportedPlatform | undefined =
  typeof window !== 'undefined'
    ? (window as unknown as { __SECOND_BRAIN_PLATFORM__?: SupportedPlatform }).__SECOND_BRAIN_PLATFORM__
    : undefined;

export const isLinux = injected?.linux ?? false;
export const isWindows = injected?.windows ?? false;
