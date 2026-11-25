import type { Conversation } from "./api";
import type { MenuItem } from "./context-menu";

export type ChatListConversation = Conversation & {
  pinned?: boolean;
  manual_unread?: boolean;
  muted?: boolean;
};

export function buildChatContextMenu(
  chat: ChatListConversation,
  options: {
    windows: boolean;
    onPin: (pinned: boolean) => void;
    onRead: (unread: boolean) => void;
    onMute: (muted: boolean) => void;
    onPopout: () => void;
    onHide: () => void;
    onDelete: () => void;
  },
): MenuItem[] {
  const pinned = Boolean(chat.pinned);
  const unread = chat.unread > 0 || Boolean(chat.manual_unread);
  const muted = Boolean(chat.muted);

  return [
    {
      label: pinned ? "取消置顶" : "置顶聊天",
      action: () => options.onPin(!pinned),
    },
    {
      label: unread ? "标为已读" : "标为未读",
      action: () => options.onRead(!unread),
    },
    {
      label: muted ? "取消免打扰" : "消息免打扰",
      action: () => options.onMute(!muted),
    },
    {
      label: "独立窗口显示",
      divider: true,
      disabled: !options.windows,
      action: options.onPopout,
    },
    {
      label: "隐藏聊天",
      action: options.onHide,
    },
    {
      label: "删除聊天",
      divider: true,
      danger: true,
      action: options.onDelete,
    },
  ];
}

export function sortConversationsForDisplay<T extends ChatListConversation>(chats: readonly T[]): T[] {
  return [...chats].sort((a, b) => {
    const pinnedDiff = Number(Boolean(b.pinned)) - Number(Boolean(a.pinned));
    if (pinnedDiff !== 0) return pinnedDiff;
    const activityDiff = b.activity_at - a.activity_at;
    if (activityDiff !== 0) return activityDiff;
    return a.id < b.id ? -1 : a.id > b.id ? 1 : 0;
  });
}
