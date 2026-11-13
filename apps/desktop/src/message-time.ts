const hour = new Intl.DateTimeFormat("en-GB", { hour: "2-digit", minute: "2-digit", hourCycle: "h23" });
const date = new Intl.DateTimeFormat(undefined, { year: "numeric", month: "short", day: "numeric" });

export function dateTimeLabel(value: number) {
  return `${date.format(value)} ${hour.format(value)}`;
}

export function messageTimeLabel(value: number, now = new Date()) {
  const day = new Date(value).toDateString();
  if (day === now.toDateString()) return hour.format(value);
  const yesterday = new Date(now);
  yesterday.setDate(yesterday.getDate() - 1);
  return day === yesterday.toDateString() ? `昨天 ${hour.format(value)}` : dateTimeLabel(value);
}

export function startsTimeGroup(value: number, start?: number) {
  return start === undefined || value - start >= 5 * 60_000
    || new Date(value).toDateString() !== new Date(start).toDateString();
}
