import assert from "node:assert/strict";
import test from "node:test";
import { buildChatContextMenu, sortConversationsForDisplay } from "../src/chat-list-menu.ts";
import { applyAppLanguage } from "../src/locale.ts";

function makeChat(overrides = {}) {
  return {
    id: "00000000-0000-0000-0000-000000000001",
    title: null,
    members: [],
    activity_at: 1000,
    preview: null,
    preview_seq: null,
    latest_seq: 0,
    unread: 0,
    read_through: 0,
    pinned: false,
    manual_unread: false,
    muted: false,
    ...overrides,
  };
}

test("sortConversationsForDisplay keeps pinned chats ahead of unpinned chats and sorts by activity_at DESC then id ASC", () => {
  const olderPinned = makeChat({ id: "00000000-0000-0000-0000-000000000010", activity_at: 100, pinned: true });
  const newerPinned = makeChat({ id: "00000000-0000-0000-0000-000000000020", activity_at: 200, pinned: true });
  const newerNormal = makeChat({ id: "00000000-0000-0000-0000-000000000005", activity_at: 999, pinned: false });
  const tieNormalB = makeChat({ id: "00000000-0000-0000-0000-000000000030", activity_at: 500, pinned: false });
  const tieNormalA = makeChat({ id: "00000000-0000-0000-0000-000000000025", activity_at: 500, pinned: false });

  const input = [tieNormalB, olderPinned, newerNormal, tieNormalA, newerPinned];
  const sorted = sortConversationsForDisplay(input);

  assert.deepEqual(
    sorted.map(chat => chat.id),
    [newerPinned.id, olderPinned.id, newerNormal.id, tieNormalA.id, tieNormalB.id],
  );
  // Pure function: input order is preserved.
  assert.deepEqual(
    input.map(chat => chat.id),
    [tieNormalB.id, olderPinned.id, newerNormal.id, tieNormalA.id, newerPinned.id],
  );

  // Search filtering preserves pinned-first order.
  const searchPool = [
    { ...newerNormal, title: "Alpha Project" },
    { ...olderPinned, title: "Alpha Team" },
    { ...tieNormalA, title: "Beta Room" },
  ];
  const filtered = sortConversationsForDisplay(searchPool).filter(chat => chat.title?.includes("Alpha"));
  assert.deepEqual(filtered.map(chat => chat.id), [olderPinned.id, newerNormal.id]);
});

test("buildChatContextMenu produces the exact 6 items without info/search items", () => {
  applyAppLanguage("zh-CN");
  const calls = [];
  const handlers = {
    windows: true,
    onPin: pinned => calls.push(["pin", pinned]),
    onRead: unread => calls.push(["read", unread]),
    onMute: muted => calls.push(["mute", muted]),
    onPopout: () => calls.push(["popout"]),
    onHide: () => calls.push(["hide"]),
    onDelete: () => calls.push(["delete"]),
  };

  const unreadChat = makeChat({ unread: 2, pinned: false, muted: false });
  const items = buildChatContextMenu(unreadChat, handlers);

  assert.equal(items.length, 6);
  assert.deepEqual(
    items.map(item => item.label),
    ["置顶聊天", "标为已读", "消息免打扰", "独立窗口显示", "隐藏聊天", "删除聊天"],
  );
  assert.equal(items[3].divider, true);
  assert.equal(items[3].disabled, false);
  assert.equal(items[5].divider, true);
  assert.equal(items[5].danger, true);
  assert.ok(!items.some(item => item.label === "聊天信息" || item.label === "查找聊天内容"));

  for (const item of items) item.action();
  assert.deepEqual(calls, [
    ["pin", true],
    ["read", false],
    ["mute", true],
    ["popout"],
    ["hide"],
    ["delete"],
  ]);

  // Verify toggled states (pinned, manual_unread only, muted) and non-Windows disabled popout.
  const toggledCalls = [];
  const manualUnreadChat = makeChat({ unread: 0, manual_unread: true, pinned: true, muted: true });
  const toggled = buildChatContextMenu(manualUnreadChat, {
    windows: false,
    onPin: pinned => toggledCalls.push(["pin", pinned]),
    onRead: unread => toggledCalls.push(["read", unread]),
    onMute: muted => toggledCalls.push(["mute", muted]),
    onPopout: () => toggledCalls.push(["popout"]),
    onHide: () => toggledCalls.push(["hide"]),
    onDelete: () => toggledCalls.push(["delete"]),
  });
  assert.deepEqual(
    toggled.map(item => item.label),
    ["取消置顶", "标为已读", "取消免打扰", "独立窗口显示", "隐藏聊天", "删除聊天"],
  );
  assert.equal(toggled[3].disabled, true);
  toggled[0].action();
  toggled[1].action();
  toggled[2].action();
  assert.deepEqual(toggledCalls, [
    ["pin", false],
    ["read", false],
    ["mute", false],
  ]);

  // Read chat shows 标为未读 and passes unread = true.
  let nextRead;
  const readMenu = buildChatContextMenu(makeChat({ unread: 0, manual_unread: false }), {
    ...handlers,
    onRead: unread => { nextRead = unread; },
  });
  assert.equal(readMenu[1].label, "标为未读");
  readMenu[1].action();
  assert.equal(nextRead, true);

  applyAppLanguage("en");
  assert.deepEqual(buildChatContextMenu(unreadChat, handlers).map(item => item.label), [
    "Pin chat", "Mark as read", "Mute chat", "Open in a new window", "Hide chat", "Delete chat",
  ]);
});
