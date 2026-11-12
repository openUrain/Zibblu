import { invoke } from "@tauri-apps/api/core";

export type AiProvider = "open_ai" | "deep_seek" | "grok";
export type Assistant = { provider: AiProvider; name: string; model: string };
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
export type AiReply = { id: string; assistant: Assistant; text: string; share: boolean };

export const aiProviders = {
  open_ai: { label: "OpenAI", name: "openai", model: "gpt-5.4-mini" },
  deep_seek: { label: "DeepSeek", name: "deepseek", model: "deepseek-flash" },
  grok: { label: "Grok (xAI)", name: "grok", model: "grok-4.7" },
} satisfies Record<AiProvider, { label: string; name: string; model: string }>;

export const ai = {
  assistants: () => invoke<Assistant[]>("ai_assistants"),
  saveAssistant: (provider: AiProvider, name: string, model: string, key?: string) =>
    invoke<Assistant[]>("ai_save_assistant", { provider, name, model, key }),
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
  send: (view: string, draft: string, approval: string) => invoke<string>("ai_send", { view, draft, approval }),
};
