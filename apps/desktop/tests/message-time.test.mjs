import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
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

  // Sub-24h / 23h / 25h intervals across local calendar day and Monday week boundaries
  assert.equal(messageTimeLabel(new Date(2026, 2, 8, 1, 30).getTime(), new Date(2026, 2, 9, 0, 30)), "昨天");
  assert.equal(messageTimeLabel(new Date(2026, 2, 11, 23, 30).getTime(), new Date(2026, 2, 15, 0, 30)), "星期三");
  assert.equal(messageTimeLabel(new Date(2026, 2, 8, 23, 30).getTime(), new Date(2026, 2, 11, 0, 30)), "3月8日");
});

test("uses local calendar day and Monday natural week across time zones and DST transitions", () => {
  const moduleUrl = new URL("../src/message-time.ts", import.meta.url).href;
  const evalInTz = (tz, script) => JSON.parse(execFileSync(
    process.execPath,
    ["--input-type=module", "-e", `import { messageTimeLabel } from ${JSON.stringify(moduleUrl)}; console.log(JSON.stringify(${script}));`],
    { env: { ...process.env, TZ: tz }, encoding: "utf8" },
  ));

  // Same UTC instants cross Monday week boundary in UTC / Asia/Shanghai, but stay in the same Sun-end week in America/New_York
  const sample = "[messageTimeLabel(Date.parse('2026-11-20T04:30:00Z'), new Date('2026-11-23T04:30:00Z')), messageTimeLabel(Date.parse('2026-11-22T23:30:00Z'), new Date('2026-11-23T01:00:00Z'))]";
  assert.deepEqual(evalInTz("Asia/Shanghai", sample), ["11月20日", "07:30"]);
  assert.deepEqual(evalInTz("UTC", sample), ["11月20日", "昨天"]);
  assert.deepEqual(evalInTz("America/New_York", sample), ["星期四", "18:30"]);

  // Real America/New_York DST transitions: 23h spring-forward (2026-03-08) and 25h fall-back (2026-11-01)
  const dstChecks = `[
    messageTimeLabel(new Date(2026, 2, 8, 1, 30).getTime(), new Date(2026, 2, 9, 1, 0)),
    messageTimeLabel(new Date(2026, 2, 8, 0, 30).getTime(), new Date(2026, 2, 12, 9, 0)),
    messageTimeLabel(new Date(2026, 2, 9, 0, 30).getTime(), new Date(2026, 2, 12, 9, 0)),
    messageTimeLabel(new Date(2026, 10, 1, 0, 30).getTime(), new Date(2026, 10, 2, 0, 30)),
    messageTimeLabel(new Date(2026, 10, 1, 0, 30).getTime(), new Date(2026, 10, 5, 9, 0)),
    messageTimeLabel(new Date(2026, 10, 2, 0, 30).getTime(), new Date(2026, 10, 5, 9, 0))
  ]`;
  assert.deepEqual(evalInTz("America/New_York", dstChecks), ["昨天", "3月8日", "星期一", "昨天", "11月1日", "星期一"]);
});

test("formats the same local calendar in English when selected", () => {
  const now = new Date(2026, 10, 29, 20);
  const format = (year, month, day) => messageTimeLabel(new Date(year, month, day, 8).getTime(), now, "en");
  assert.equal(format(2026, 10, 29), "08:00");
  assert.equal(format(2026, 10, 28), "Yesterday");
  assert.equal(format(2026, 10, 27), "Day before yesterday");
  assert.equal(format(2026, 10, 26), "Thursday");
  assert.equal(format(2026, 10, 22), "Nov 22");
  assert.equal(format(2025, 10, 22), "Nov 22, 2025");
});
