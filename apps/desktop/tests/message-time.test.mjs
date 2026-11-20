import assert from "node:assert/strict";
import test from "node:test";
import { messageTimeLabel, startsTimeGroup } from "../src/message-time.ts";

test("groups time by the immediately preceding message", () => {
  const start = new Date(2026, 10, 14, 10).getTime();
  assert.equal(startsTimeGroup(start), true);
  assert.equal(startsTimeGroup(start + 599_000, start), false);
  assert.equal(startsTimeGroup(start + 600_000, start), false);
  assert.equal(startsTimeGroup(start + 601_000, start), true);

  const times = [0, 4, 8, 12, 18, 29].map(minutes => start + minutes * 60_000);
  assert.deepEqual(times.map((time, index) => startsTimeGroup(time, times[index - 1])),
    [true, false, false, false, false, true]);
});

test("starts a group across local midnight and preserves local labels", () => {
  const before = new Date(2026, 10, 14, 23, 59).getTime();
  const after = new Date(2026, 10, 15, 0, 1);
  assert.equal(startsTimeGroup(after.getTime(), before), true);
  assert.equal(messageTimeLabel(after.getTime(), after), "00:01");
  assert.equal(messageTimeLabel(before, after), "昨天");
  assert.equal(messageTimeLabel(before, new Date(2026, 10, 16)), "前天");
  assert.equal(messageTimeLabel(before, new Date(2026, 10, 20)), "星期六");
  assert.equal(messageTimeLabel(before, new Date(2026, 10, 21)), "11月14日");
  assert.equal(messageTimeLabel(before, new Date(2027, 0, 1)), "2026年11月14日");
  assert.equal(messageTimeLabel(new Date(2026, 11, 31, 23).getTime(), new Date(2027, 0, 1)), "昨天");
});
