import assert from "node:assert/strict";
import test from "node:test";
import { setImmediate } from "node:timers/promises";
import { ImageCache, ProfileCache } from "../src/workspace-cache.ts";

const key = "alice/contact";
const snapshot = (name, expires_at = Date.now() + 86_400_000) => ({
  profile: { display_name: name },
  expires_at,
});

test("shows local data first, coalesces concurrent reads and checks only changed or stale profiles", async () => {
  const cache = new ProfileCache();
  let localReads = 0;
  let requests = 0;
  let reply;
  const read = async () => { localReads++; return snapshot("Alice"); };
  const verify = () => { requests++; return new Promise(resolve => { reply = resolve; }); };
  const detail = cache.load(key, "v1", read, verify);
  const card = cache.load(key, "v1", read, verify);
  await setImmediate();
  const original = cache.get(key).profile;
  assert.equal(original.display_name, "Alice");
  reply(snapshot("Alice"));
  await Promise.all([detail, card]);
  assert.equal(cache.get(key).profile, original);
  await cache.load(key, "v1", read, verify);
  assert.deepEqual([localReads, requests], [1, 1]);
  const changed = cache.load(key, "v2", read, verify);
  assert.equal(cache.get(key).profile, original);
  reply(snapshot("Updated Alice"));
  await changed;
  assert.equal(cache.get(key).profile.display_name, "Updated Alice");
  assert.deepEqual([localReads, requests], [1, 2]);
});

test("expired snapshots stay hidden offline and a revoked contact cannot be restored by an old response", async () => {
  const cache = new ProfileCache();
  const offline = new Error("offline");
  await assert.rejects(cache.load(key, "v1", async () => snapshot("Expired", 0), async () => { throw offline; }), offline);
  assert.equal(cache.get(key), undefined);
  let outdated;
  const pending = cache.load(key, "v1", async () => snapshot("Cached"),
    () => new Promise(resolve => { outdated = resolve; }));
  await setImmediate();
  assert.equal(cache.get(key).profile.display_name, "Cached");
  cache.retain(() => false);
  outdated(snapshot("Outdated"));
  await pending;
  assert.equal(cache.get(key), undefined);
});

test("a second window's directory version cannot install an earlier in-flight profile", async () => {
  const cache = new ProfileCache();
  let reply;
  const older = cache.load(key, "avatar-v1", async () => snapshot("Cached"), () => new Promise(resolve => { reply = resolve; }));
  await setImmediate();
  const newer = cache.load(key, "avatar-v2", async () => null, async () => snapshot("New"));
  reply(snapshot("Old"));
  await older;
  assert.notEqual(cache.get(key).profile.display_name, "Old");
  await newer;
  assert.equal(cache.get(key).profile.display_name, "New");
});

test("open, switch and return reuse one thumbnail read and one decode until revoked", async () => {
  let decodes = 0;
  const browserImage = globalThis.Image;
  globalThis.Image = class { async decode() { decodes++; } };
  try {
    const cache = new ImageCache();
    const media = "thumbnail/chat-a/1/attachment/hash";
    let reads = 0;
    const fetch = async () => { reads++; return new Uint8Array([1, 2]).buffer; };
    const a = cache.acquire(media, Infinity, fetch);
    const aAgain = cache.acquire(media, Infinity, fetch);
    assert.equal(await a.ready, await aAgain.ready);
    a.release();
    aAgain.release();
    const returning = cache.acquire(media, Infinity, fetch);
    assert.equal(await returning.ready, cache.get(media));
    returning.release();
    assert.deepEqual([reads, decodes], [1, 1]);
    cache.retain(() => false);
    assert.equal(cache.get(media), undefined);
  } finally {
    globalThis.Image = browserImage;
  }
});
