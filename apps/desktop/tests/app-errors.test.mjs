import assert from "node:assert/strict";
import test from "node:test";
import { chineseErrorMessage } from "../src/app-errors.ts";

test("keeps save replacement recovery instructions in Chinese", () => {
  assert.equal(
    chineseErrorMessage("The destination changed. Choose the save location and confirm replacement again."),
    "目标文件已发生变化。请重新选择保存位置并确认覆盖。",
  );
  assert.match(
    chineseErrorMessage("The save destination already exists. Choose the location again to confirm replacement."),
    /重新选择保存位置并确认覆盖/,
  );
});

test("preserves unmapped diagnostic details", () => {
  const diagnostic = "The server needs a newer client. Update and retry.";
  assert.equal(chineseErrorMessage(diagnostic), diagnostic);
});
