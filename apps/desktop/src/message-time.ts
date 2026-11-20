function calendarDay(date: Date) {
  return Date.UTC(date.getFullYear(), date.getMonth(), date.getDate()) / 86_400_000;
}

export function messageTimeLabel(value: number, now = new Date()) {
  const date = new Date(value);
  const days = calendarDay(now) - calendarDay(date);
  if (days === 0) return [date.getHours(), date.getMinutes()].map(n => String(n).padStart(2, "0")).join(":");
  if (days === 1) return "昨天";
  if (days === 2) return "前天";
  if (days >= 3 && days < 7) return "星期" + "日一二三四五六"[date.getDay()];
  const monthDay = (date.getMonth() + 1) + "月" + date.getDate() + "日";
  return date.getFullYear() === now.getFullYear() ? monthDay : date.getFullYear() + "年" + monthDay;
}

export const dateTimeLabel = messageTimeLabel;

export function startsTimeGroup(value: number, previous?: number) {
  return previous === undefined || value - previous > 10 * 60_000
    || new Date(value).toDateString() !== new Date(previous).toDateString();
}
