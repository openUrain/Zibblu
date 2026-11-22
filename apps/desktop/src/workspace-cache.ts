import type { CachedProfile, Profile } from "./api";

const profileCheckInterval = 60_000;
const profileLimit = 256;
const imageLimit = 128;
const imageBudget = 24 * 1024 * 1024;

type ProfileEntry = {
  value?: CachedProfile;
  checked?: { at: number; version: string };
  wanted?: string;
  task?: Promise<void>;
};

export class ProfileCache {
  private readonly entries = new Map<string, ProfileEntry>();
  private readonly listeners = new Set<() => void>();

  subscribe = (listener: () => void) => {
    this.listeners.add(listener);
    return () => { this.listeners.delete(listener); };
  };

  get(key: string) {
    return this.entries.get(key)?.value;
  }

  load(key: string, version: string, readLocal: () => Promise<CachedProfile | null>,
    verify: () => Promise<CachedProfile>, force = false): Promise<void> {
    const entry = this.entries.get(key) ?? {};
    this.entries.delete(key);
    this.entries.set(key, entry);
    entry.wanted = version;
    this.trim();
    if (entry.task) {
      if (entry.checked?.version === version) return entry.task;
      return entry.task.catch(() => {}).then(() => {
        if (this.entries.get(key) === entry && entry.wanted === version) {
          return this.load(key, version, readLocal, verify, force);
        }
      });
    }
    if (!force && entry.value && entry.value.expires_at > Date.now()
      && entry.checked?.version === version && entry.checked.at + profileCheckInterval > Date.now()) return Promise.resolve();
    const current = () => this.entries.get(key) === entry && entry.wanted === version;
    const set = (value: CachedProfile | undefined) => {
      if (!current() || entry.value === value) return;
      entry.value = value;
      this.notify();
    };
    entry.checked = { at: 0, version };
    entry.task = (async () => {
      if (!entry.value || entry.value.expires_at <= Date.now()) {
        const local = await readLocal().catch(() => null);
        if (!current()) return;
        set(local && local.expires_at > Date.now() ? local : undefined);
      }
      try {
        const result = await verify();
        if (!current()) return;
        const previous = entry.value?.profile;
        const unchanged = previous && JSON.stringify(previous) === JSON.stringify(result.profile);
        set(unchanged ? { ...result, profile: previous } : result);
        entry.checked = { at: Date.now(), version };
      } catch (error) {
        if (current() && typeof error === "object" && error && "status" in error
          && (error.status === 401 || error.status === 403 || error.status === 404)) set(undefined);
        throw error;
      }
    })().finally(() => {
      entry.task = undefined;
      this.trim();
    });
    return entry.task;
  }

  patch(id: string, patch: Partial<Profile>) {
    for (const [key, entry] of this.entries) {
      if (!key.startsWith(id + "/")) continue;
      if (!entry.value) {
        this.entries.delete(key);
        continue;
      }
      this.entries.set(key, { value: { ...entry.value, profile: { ...entry.value.profile, ...patch } } });
    }
    this.notify();
  }

  retain(allowed: (key: string) => boolean) {
    let changed = false;
    for (const key of this.entries.keys()) {
      if (!allowed(key)) changed = this.entries.delete(key) || changed;
    }
    if (changed) this.notify();
  }

  private trim() {
    for (const [key, entry] of this.entries) {
      if (this.entries.size <= profileLimit) break;
      if (!entry.task) this.entries.delete(key);
    }
  }

  private notify() {
    for (const listener of this.listeners) listener();
  }
}

type ImageEntry = { url?: string; bytes: number; expires: number; refs: number; task?: Promise<string | undefined> };

export class ImageCache {
  private readonly entries = new Map<string, ImageEntry>();
  private bytes = 0;

  get(key: string) {
    const entry = this.entries.get(key);
    return entry && entry.expires > Date.now() ? entry.url : undefined;
  }

  acquire(key: string, expires: number, read: () => Promise<ArrayBuffer>) {
    let entry = this.entries.get(key);
    if (entry && entry.expires <= Date.now()) {
      this.remove(key, entry);
      entry = undefined;
    }
    const active: ImageEntry = entry ?? { bytes: 0, refs: 0, expires };
    active.refs += 1;
    active.expires = expires;
    this.entries.delete(key);
    this.entries.set(key, active);
    if (!active.url && !active.task) {
      active.task = read().then(async bytes => {
        const url = URL.createObjectURL(new Blob([bytes]));
        try {
          const image = new Image();
          image.src = url;
          await image.decode();
          if (this.entries.get(key) === active && active.expires > Date.now()) {
            active.url = url;
            active.bytes = bytes.byteLength;
            this.bytes += active.bytes;
            return url;
          }
        } finally {
          if (active.url !== url) URL.revokeObjectURL(url);
        }
      }).catch(error => {
        if (this.entries.get(key) === active) this.remove(key, active);
        throw error;
      }).finally(() => {
        active.task = undefined;
        this.trim();
      });
    }
    return {
      ready: active.task ?? Promise.resolve(active.url),
      release: () => {
        active.refs -= 1;
        this.trim();
      },
    };
  }

  retain(allowed: (key: string) => boolean) {
    for (const [key, entry] of this.entries) {
      if (entry.expires <= Date.now() || !allowed(key)) this.remove(key, entry);
    }
  }

  private remove(key: string, entry: ImageEntry) {
    if (entry.url) URL.revokeObjectURL(entry.url);
    this.bytes -= entry.bytes;
    this.entries.delete(key);
  }

  private trim() {
    for (const [key, entry] of this.entries) {
      if (this.entries.size <= imageLimit && this.bytes <= imageBudget) break;
      if (!entry.refs && !entry.task) this.remove(key, entry);
    }
  }
}
