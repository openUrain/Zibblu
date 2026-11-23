import assert from "node:assert/strict";
import test from "node:test";
import { dateTimeLabel, messageTimeLabel, startsTimeGroup } from "../src/message-time.ts";

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

test("starts a group across local midnight and formats Monday-start natural week labels", () => {
  assert.equal(dateTimeLabel, messageTimeLabel);

  const before = new Date(2026, 10, 14, 23, 59).getTime();
  const after = new Date(2026, 10, 15, 0, 1);
  assert.equal(startsTimeGroup(after.getTime(), before), true);
  assert.equal(messageTimeLabel(after.getTime(), after), "00:01");
  assert.equal(messageTimeLabel(before, after), "昨天");
  assert.equal(messageTimeLabel(before, new Date(2026, 10, 16)), "前天");
  assert.equal(messageTimeLabel(before, new Date(2026, 10, 17)), "11月14日");
  assert.equal(messageTimeLabel(before, new Date(2027, 0, 1)), "2026年11月14日");

  const mondayNow = new Date(2026, 10, 23, 18, 0);
  assert.equal(messageTimeLabel(new Date(2026, 10, 23, 12, 30).getTime(), mondayNow), "12:30");
  assert.equal(messageTimeLabel(new Date(2026, 10, 22, 21, 0).getTime(), mondayNow), "昨天");
  assert.equal(messageTimeLabel(new Date(2026, 10, 21, 9, 0).getTime(), mondayNow), "前天");
  assert.equal(messageTimeLabel(new Date(2026, 10, 20, 23, 59).getTime(), mondayNow), "11月20日");
  assert.equal(messageTimeLabel(new Date(2026, 10, 17, 8, 15).getTime(), mondayNow), "11月17日");

  const thursdayNow = new Date(2026, 10, 26, 9, 0);
  assert.equal(messageTimeLabel(new Date(2026, 10, 23, 23, 59).getTime(), thursdayNow), "星期一");
  assert.equal(messageTimeLabel(new Date(2026, 10, 24, 0, 1).getTime(), thursdayNow), "前天");
  assert.equal(messageTimeLabel(new Date(2026, 10, 25, 22, 0).getTime(), thursdayNow), "昨天");

  const sundayNow = new Date(2026, 10, 29, 20, 0);
  assert.equal(messageTimeLabel(new Date(2026, 10, 26, 10, 0).getTime(), sundayNow), "星期四");
  assert.equal(messageTimeLabel(new Date(2026, 10, 23, 0, 0).getTime(), sundayNow), "星期一");
  assert.equal(messageTimeLabel(new Date(2026, 10, 22, 23, 59).getTime(), sundayNow), "11月22日");

  // Cross-month within the same Monday-start natural week (Mon 2026-11-30 to Fri 2026-12-04)
  const crossMonthNow = new Date(2026, 11, 4, 10, 0);
  assert.equal(messageTimeLabel(new Date(2026, 10, 30, 8, 0).getTime(), crossMonthNow), "星期一");
  assert.equal(messageTimeLabel(new Date(2026, 10, 29, 23, 59).getTime(), crossMonthNow), "11月29日");

  // Cross-year boundaries (Thu 2026-12-31 vs Fri 2027-01-01 and Sun 2027-01-03)
  assert.equal(messageTimeLabel(new Date(2026, 11, 31, 23).getTime(), new Date(2027, 0, 1)), "昨天");
  assert.equal(messageTimeLabel(new Date(2026, 11, 31, 9).getTime(), new Date(2027, 0, 2)), "前天");
  assert.equal(messageTimeLabel(new Date(2026, 11, 28, 9).getTime(), new Date(2027, 0, 3)), "星期一");
  assert.equal(messageTimeLabel(new Date(2026, 11, 27, 23).getTime(), new Date(2027, 0, 3)), "2026年12月27日");

  // Sub-24h / 23h / 25h DST-like intervals across local calendar day and Monday week boundaries
  assert.equal(messageTimeLabel(new Date(2026, 2, 8, 1, 30).getTime(), new Date(2026, 2, 9, 0, 30)), "昨天");
  assert.equal(messageTimeLabel(new Date(2026, 2, 11, 23, 30).getTime(), new Date(2026, 2, 15, 0, 30)), "星期三");
  assert.equal(messageTimeLabel(new Date(2026, 2, 8, 23, 30).getTime(), new Date(2026, 2, 11, 0, 30)), "3月8日");
});
