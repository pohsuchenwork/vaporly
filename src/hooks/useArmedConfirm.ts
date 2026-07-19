import { useCallback, useEffect, useRef, useState } from "react";

/**
 * Two-step confirmation for destructive actions: the first trigger arms the
 * control (caller renders it red / relabeled), a second trigger within the
 * window executes. Auto-disarms after `timeoutMs` so a stray click never
 * leaves a live trigger behind.
 */
export function useArmedConfirm(action: () => void, timeoutMs = 3000) {
  const [armed, setArmed] = useState(false);
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);

  useEffect(
    () => () => {
      if (timer.current) clearTimeout(timer.current);
    },
    [],
  );

  const fire = useCallback(() => {
    if (armed) {
      if (timer.current) clearTimeout(timer.current);
      setArmed(false);
      action();
      return;
    }
    setArmed(true);
    if (timer.current) clearTimeout(timer.current);
    timer.current = setTimeout(() => setArmed(false), timeoutMs);
  }, [armed, action, timeoutMs]);

  return { armed, fire };
}
