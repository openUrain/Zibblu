import assert from "node:assert/strict";
import test from "node:test";
import { layoutAnnotationText, renderAnnotationDoc, resizeTextByHandle, textOutsideImage } from "../src/image-annotator.ts";

test("font changes and narrow Chinese text boxes keep all lines in the export canvas", () => {
  const item = {
    id: "text", kind: "text", text: "一段很长的中文标注用于复查\n末尾不能丢失",
    x: 0, y: 0, width: 80, height: 8, fontSize: 16, family: "Microsoft YaHei",
    color: "#1D1D1F",
  };
  const drawn = [];
  const ctx = {
    font: "default",
    save() {}, restore() {}, translate() {}, rotate() {},
    measureText(text) { return { width: [...text].length * (this.font.includes("SimSun") ? 12 : 16) }; },
    fillText(line, x, y) { drawn.push({ line, x, y, font: this.font }); },
  };
  const resized = resizeTextByHandle(item, "e", { x: 48, y: 0 }, { x: 80, y: 0 });
  assert.equal(resized.fontSize, item.fontSize);
  assert.equal(resized.width, 48);
  const shortened = resizeTextByHandle(resized, "s", { x: 0, y: 4 }, { x: 0, y: 80 });
  const changed = { ...shortened, family: "SimSun" };
  const expected = layoutAnnotationText(changed, ctx);
  assert.ok(expected.height >= expected.lines.length * expected.lineHeight);
  assert.notDeepEqual(expected.lines, layoutAnnotationText(shortened, ctx).lines);
  assert.equal(expected.lines.join(""), item.text.replace("\n", ""));
  renderAnnotationDoc(ctx, { naturalWidth: 500, naturalHeight: 500, items: [changed] });
  assert.deepEqual(drawn.map(line => line.line), expected.lines);
  assert.ok(drawn.every(({ y, font }) => y < expected.height && font.includes('"SimSun"')));
  assert.ok(textOutsideImage({ ...changed, y: 480 }, 500, 500));
  assert.ok(!textOutsideImage(changed, 500, 500));
});
