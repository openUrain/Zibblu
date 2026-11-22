import { createContext, useContext, useEffect, useState } from "react";
import { api } from "./api";
import type { Attachment, Message } from "./api";
import type { ImageCache } from "./workspace-cache";

export const WorkspaceImages = createContext<{
  cache: ImageCache; clockOffset: number; canPreview: (conversation: string, seq: number) => boolean;
} | undefined>(undefined);

export function thumbnailKey(message: Message, file: Attachment) {
  return `thumbnail/${message.conversation_id}/${message.seq}/${file.id}/${file.sha256}`;
}

export function useWorkspaceImage(key: string | undefined, id: string, version?: string, expires = Infinity, retry = 0, visible = true) {
  const cache = useContext(WorkspaceImages)?.cache;
  const [result, setResult] = useState<{ key: string; retry: number; failed: boolean }>();
  const [, setElapsed] = useState(0);
  useEffect(() => {
    if (!key || !cache || !visible || expires <= Date.now()) return;
    let current = true;
    const entry = cache.acquire(key, expires, () => api.previewImageBytes(id, version));
    void entry.ready.then(url => {
      if (current && url) setResult({ key, retry, failed: false });
    }).catch(() => {
      if (current) setResult({ key, retry, failed: true });
    });
    return () => { current = false; entry.release(); };
  }, [cache, key, id, version, expires, retry, visible]);
  useEffect(() => {
    if (!Number.isFinite(expires) || !key) return;
    let timer: number;
    function check() {
      const remaining = expires - Date.now();
      if (remaining > 0) {
        timer = window.setTimeout(check, Math.min(2_147_483_647, remaining));
        return;
      }
      cache?.retain(candidate => candidate !== key);
      setElapsed(value => value + 1);
    }
    check();
    return () => window.clearTimeout(timer);
  }, [cache, key, expires]);
  const expired = expires <= Date.now();
  return {
    url: !expired && key ? cache?.get(key) : undefined,
    failed: expired || (result?.key === key && result?.retry === retry && result.failed),
  };
}
