import assert from "node:assert/strict";
import test from "node:test";
import { canPreview, mediaKind, readAttachmentPage } from "../src/attachment-media.ts";

test("media stays in chat history even without an inline decoder", () => {
  const file = (name, size = 100, kind = "file") => ({ kind, name, size });
  for (const [name, kind, preview] of [
    ["picture.PNG", "image", true], ["animation.apng", "image", true], ["photo.jpeg", "image", true],
    ["sticker.webp", "image", true], ["animation.gif", "image", true], ["picture.bmp", "image", true],
    ["clip.webm", "video", true], ["photo.avif", "image", false],
    ["photo.heic", "image", false], ["clip.avi", "video", false],
    ["report.pdf", undefined, false], ["png", undefined, false],
  ]) {
    assert.equal(mediaKind(file(name)), kind);
    assert.equal(canPreview(file(name)), preview);
  }
  assert.equal(canPreview(file("picture.png", 0)), false);
  assert.equal(canPreview(file("picture.png", 16 * 1024 * 1024 + 1)), false);
  assert.equal(mediaKind(file("folder.png", 100, "folder")), undefined);
  assert.equal(canPreview(file("folder.png", 100, "folder")), false);
});

test("skips fully filtered pages without losing ordinary files in either order", async () => {
  const message = (seq, ext) => ({ seq, attachment: { kind: "file", name: `sample.${ext}` } });
  const mixed = [message(17, "png"), message(22, "png"), message(13, "txt"), message(30, "jpg"),
    message(19, "png"), message(25, "png"), message(26, "avi"), message(32, "zip"), message(14, "png"), message(27, "png")];
  for (const oldest of [true, false]) {
    const order = oldest ? mixed : [...mixed].reverse();
    const read = async before => {
      const start = before === undefined ? 0 : order.findIndex(item => item.seq === before) + 1;
      return { items: order.slice(start, start + 2), has_more: start + 2 < order.length };
    };
    const first = await readAttachmentPage(read, false, new Set(), new Set(), undefined, 1);
    assert.deepEqual(first.items.map(item => item.seq), oldest ? [13] : [32]);
    assert.equal(first.has_more, true);
    const second = await readAttachmentPage(read, false, new Set(), new Set(), first.items[0].seq, 1);
    assert.deepEqual(second.items.map(item => item.seq), oldest ? [32] : [13]);
    assert.equal(second.has_more, false);
    const media = await readAttachmentPage(read, true, new Set([22]), new Set([19]));
    assert.deepEqual(media.items.map(item => item.seq),
      order.filter(item => ![13, 19, 22, 32].includes(item.seq)).map(item => item.seq));
  }
});
