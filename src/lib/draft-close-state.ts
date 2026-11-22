let nativeClosePending = false;

export function isNativeDraftClosePending() {
  return nativeClosePending;
}

export function setNativeDraftClosePending(pending: boolean) {
  nativeClosePending = pending;
}
