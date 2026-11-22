import { useEffect, useState } from "react";
import { startPolling } from "@/lib/poll";
export function useClock(interval = 30_000, enabled = true) {
  const [now, setNow] = useState(Date.now);
  useEffect(() => {
    if (!enabled) return;
    const tick = () => {
      if (!document.hidden) setNow(Date.now());
    };
    const stop = startPolling(async () => tick(), interval);
    window.addEventListener("focus", tick);
    document.addEventListener("visibilitychange", tick);
    return () => {
      stop();
      window.removeEventListener("focus", tick);
      document.removeEventListener("visibilitychange", tick);
    };
  }, [interval, enabled]);
  return now;
}
