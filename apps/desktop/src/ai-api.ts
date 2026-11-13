import { invoke } from "@tauri-apps/api/core";

export type AiProvider = "open_ai" | "deep_seek" | "grok";
export type Assistant = { provider: AiProvider; name: string; model: string; base_url?: string | null };
export type AiFile = { id: string; name: string; bytes: number };
export type AiScope =
  | { kind: "prompt" }
  | { kind: "selected"; sequences: number[] }
  | { kind: "recent"; limit: number };
export type AiPreview = {
  id: string;
  endpoint: string;
  assistant: Assistant;
  snapshot: {
    prompt: string;
    messages: { sender: string; text: string }[];
    files: { name: string; text: string }[];
  };
};
export type AiReply = { id: string; conversation: string; provider: AiProvider; text: string; createdAt: number };

export const aiProviders = {
  open_ai: { label: "OpenAI", name: "openai", model: "gpt-5.4-mini", base: "https://api.openai.com/v1/", icon: "openai.svg" },
  deep_seek: { label: "DeepSeek", name: "deepseek", model: "deepseek-flash", base: "https://api.deepseek.com/", icon: "deepseek.svg" },
  grok: { label: "Grok (xAI)", name: "grok", model: "grok-4.7", base: "https://api.x.ai/v1/", icon: "grok.svg" },
} satisfies Record<AiProvider, { label: string; name: string; model: string; base: string; icon: string }>;

export const ai = {
  assistants: () => invoke<Assistant[]>("ai_assistants"),
  saveAssistant: (provider: AiProvider, name: string, model: string, baseUrl: string, key?: string) =>
    invoke<Assistant[]>("ai_save_assistant", { provider, name, model, baseUrl, key }),
  testConnection: (provider: AiProvider, baseUrl: string, key?: string) =>
    invoke<{ authenticated: boolean; models: string[] }>("ai_test_connection", { provider, baseUrl, key }),
  removeAssistant: (provider: AiProvider) =>
    invoke<Assistant[]>("ai_remove_assistant", { provider }),
  openView: (view: string, conversation: string) => invoke<void>("ai_open_view", { view, conversation }),
  closeView: (view: string) => invoke<void>("ai_close_view", { view }),
  begin: (view: string, draft: string) => invoke<void>("ai_begin", { view, draft }),
  cancel: (view: string, draft: string) => invoke<void>("ai_cancel", { view, draft }),
  pickFile: (view: string, draft: string) => invoke<AiFile | null>("ai_pick_file", { view, draft }),
  removeFile: (view: string, draft: string, id: string) => invoke<void>("ai_remove_file", { view, draft, id }),
  prepare: (view: string, draft: string, selection: { provider: AiProvider; prompt: string; scope: AiScope; files: string[] }) =>
    invoke<AiPreview>("ai_prepare", { view, draft, selection }),
  send: (view: string, draft: string, approval: string) =>
    invoke<{ text: string; created_at: number }>("ai_send", { view, draft, approval }),
};
