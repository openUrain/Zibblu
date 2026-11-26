import { invoke } from "@tauri-apps/api/core";
import type { Message } from "./api";

export type AiProvider = "open_ai" | "deep_seek" | "grok";
export type Assistant = { provider: AiProvider; name: string; model: string; base_url?: string | null; agent_enabled?: boolean };
export type AiFile = { id: string; name: string; bytes: number };
export type AiScope =
  | { kind: "prompt" }
  | { kind: "selected"; sequences: number[] }
  | { kind: "recent"; limit: number };
export type AiAttachmentPreview = {
  seq: number;
  id: string;
  name: string;
  kind: "file" | "folder";
  file_type: string;
  size: number;
  cached: boolean;
};
export type AiPreview = {
  id: string;
  endpoint: string;
  assistant: Assistant;
  snapshot: {
    prompt: string;
    messages: { sender: string; text: string; attachment?: string }[];
    attachments: AiAttachmentPreview[];
    files: { name: string; text: string }[];
  };
};
export type AiPrompt = {
  id: string;
  conversation_id: string;
  provider: AiProvider;
  prompt_text: string;
  created_at: number;
};
export type AiJob = {
  id: string;
  prompt_id: string;
  conversation_id: string;
  provider: AiProvider;
  phase: "downloading" | "preparing" | "searching" | "generating" | "failed";
  status_text: string;
  progress?: [number, number] | null;
  error?: string | null;
  created_at: number;
  access?: { id: string; name: string; endpoint: string } | null;
};
export type AiSource = { number: number; conversation_id: string; seq: number; attachment_id?: string | null; label: string; created_at: number };
export type AiReply = {
  id: string;
  prompt_id: string;
  conversation_id: string;
  provider: AiProvider;
  text: string;
  created_at: number;
  sources?: AiSource[];
};

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
  enableAgent: (assistant: Assistant, enabled: boolean) =>
    invoke<Assistant[]>("ai_set_agent_enabled", { provider: assistant.provider, model: assistant.model, baseUrl: assistant.base_url, enabled }),
  startAgent: (view: string, provider: AiProvider, prompt: string) =>
    invoke<{ id: string; created_at: number }>("ai_start_agent", { view, provider, prompt }),
  answerAccess: (jobId: string, requestId: string, accept: boolean) =>
    invoke<void>("ai_answer_access", { jobId, requestId, accept }),
  source: (replyId: string, number: number) => invoke<Message>("ai_source", { replyId, number }),
  sharedSource: (conversation: string, replySeq: number, number: number) =>
    invoke<Message>("ai_shared_source", { conversation, replySeq, number }),
  openView: (view: string, conversation: string) => invoke<void>("ai_open_view", { view, conversation }),
  closeView: (view: string) => invoke<void>("ai_close_view", { view }),
  begin: (view: string, draft: string) => invoke<void>("ai_begin", { view, draft }),
  cancel: (view: string, draft: string) => invoke<void>("ai_cancel", { view, draft }),
  pickFile: (view: string, draft: string) => invoke<AiFile | null>("ai_pick_file", { view, draft }),
  removeFile: (view: string, draft: string, id: string) => invoke<void>("ai_remove_file", { view, draft, id }),
  prepare: (view: string, draft: string, selection: { provider: AiProvider; prompt: string; scope: AiScope; files: string[] }) =>
    invoke<AiPreview>("ai_prepare", { view, draft, selection }),
  send: (view: string, draft: string, approval: string) =>
    invoke<{ id: string; created_at: number }>("ai_send", { view, draft, approval }),
  prompts: (conversation: string) => invoke<AiPrompt[]>("ai_prompts", { conversation }),
  jobs: (conversation: string) => invoke<AiJob[]>("ai_jobs", { conversation }),
  replies: (conversation: string) => invoke<AiReply[]>("ai_replies", { conversation }),
  cancelJob: (jobId: string) => invoke<void>("ai_cancel_job", { jobId }),
  dismissJob: (jobId: string) => invoke<void>("ai_dismiss_job", { jobId }),
  dismissReply: (id: string) => invoke<void>("ai_dismiss_reply", { id }),
};
