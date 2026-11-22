/** Schedule after completion, never overlap, and suspend reads in hidden windows. */
export function startPolling(
  task: () => Promise<unknown>,
  interval: number,
  immediate = true,
) {
  let stopped = false;
  let running = false;
  let timer: ReturnType<typeof setTimeout> | undefined;
  const run = async () => {
    if (stopped || running || document.hidden) return;
    if (timer) clearTimeout(timer);
    running = true;
    try {
      await task();
    } finally {
      running = false;
      if (!stopped && !document.hidden)
        timer = setTimeout(() => void run(), interval);
    }
  };
  const visibility = () => {
    if (timer) clearTimeout(timer);
    if (!document.hidden) void run();
  };
  document.addEventListener("visibilitychange", visibility);
  if (immediate) void run();
  else if (!document.hidden) timer = setTimeout(() => void run(), interval);
  return () => {
    stopped = true;
    if (timer) clearTimeout(timer);
    document.removeEventListener("visibilitychange", visibility);
  };
}
