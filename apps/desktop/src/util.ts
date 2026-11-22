import { useState } from "react";

export function plural(n: number, w: string) {
  return `${n} ${w}${n === 1 ? "" : "s"}`;
}

/** Copies text; `copied` names the last copied key for 1.5 s. */
export function useCopy() {
  const [copied, setCopied] = useState<string | null>(null);
  const copy = async (key: string, text: string) => {
    await navigator.clipboard.writeText(text);
    setCopied(key);
    setTimeout(() => setCopied(null), 1500);
  };
  return { copied, copy };
}
