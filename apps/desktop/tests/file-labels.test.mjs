import assert from "node:assert/strict";
import test from "node:test";
import { decimalSize, splitFileName } from "../src/file-labels.ts";

test("only the file name stem is eligible for ordinary truncation", () => {
  for (const [name, stem, suffix] of [
    ["Online Verification Report.pdf", "Online Verification Report", ".pdf"],
    ["Backup.tar.gz", "Backup", ".tar.gz"],
    [".gitignore", ".gitignore", ""],
    ["a.", "a.", ""],
    ["无扩展名", "无扩展名", ""],
  ]) assert.deepEqual(splitFileName(name), [stem, suffix]);
});

test("file centre uses decimal units", () => {
  assert.equal(decimalSize(999), "999 B");
  assert.equal(decimalSize(1000), "1.0 KB");
  assert.equal(decimalSize(2_200_000), "2.2 MB");
  assert.equal(decimalSize(1_000_000_000), "1.0 GB");
});
