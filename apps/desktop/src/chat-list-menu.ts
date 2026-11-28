import { builtinAssistantId } from "./ai-api";
import type { Conversation } from "./api";
import type { MenuItem } from "./context-menu";
import { t } from "./locale";

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
      label: pinned ? t("Unpin chat", "取消置顶") : t("Pin chat", "置顶聊天"),
      action: () => options.onPin(!pinned),
    },
    ...(chat.id === builtinAssistantId ? [] : [{
      label: unread ? t("Mark as read", "标为已读") : t("Mark as unread", "标为未读"),
      action: () => options.onRead(!unread),
    },
    {
      label: muted ? t("Unmute chat", "取消免打扰") : t("Mute chat", "消息免打扰"),
      action: () => options.onMute(!muted),
    }]),
    {
      label: t("Open in a new window", "独立窗口显示"),
      divider: true,
      disabled: !options.windows,
      action: options.onPopout,
    },
    ...(chat.id === builtinAssistantId ? [] : [{ label: t("Hide chat", "隐藏聊天"), action: options.onHide }]),
    {
      label: t("Delete chat", "删除聊天"),
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
