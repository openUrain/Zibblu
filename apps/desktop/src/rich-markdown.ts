import type { Element, Root as HtmlRoot } from "hast";
import { fromHtmlIsomorphic } from "hast-util-from-html-isomorphic";
import katex from "katex";
import type { Nodes, Root } from "mdast";
import remarkBreaks from "remark-breaks";
import remarkGfm from "remark-gfm";
import remarkMath from "remark-math";
import remarkParse from "remark-parse";
import remarkRehype from "remark-rehype";
import { unified } from "unified";
import { messageMath } from "./rich-math";

const parser = unified().use(remarkParse).use(remarkGfm).use(remarkMath, { singleDollarTextMath: false })
  .use(messageMath).use(remarkBreaks).freeze();

const renderer = unified().use(remarkRehype, {
  handlers: {
    html: (_state, node) => node.type === "html" ? { type: "text", value: node.value } : undefined,
    math: (_state, node) => formula(node, true),
    inlineMath: (_state, node) => formula(node, node.data?.hName === "div"),
  },
}).freeze();

type ParsedMessage = { source: string; tree?: Root; rendered?: HtmlRoot | null; summary?: string };
// The timeline and its existing history cache determine the lifetime of these keys.
const normalMessages = new WeakMap<object, ParsedMessage>();
const maxSource = 64 * 1024;
const maxNodes = 4096;
const maxSummary = 140;
const markdownHints = /[\\`_*~#[\]<>$&|]|(?:^|\n)(?:[ \t]{2}|[-+]|\d{1,9}[.)] )|https?:\/\/|www\./i;

export function safeWebUrl(source: string) {
  if (!/^https?:\/\//i.test(source) || source.length > 4096) return undefined;
  try {
    const url = new URL(source);
    return !url.username && !url.password && url.href.length <= 4096 ? url.href : undefined;
  } catch {
    return undefined;
  }
}

function formula(node: Nodes, displayMode: boolean): Element | undefined {
  if (node.type !== "math" && node.type !== "inlineMath") return;
  const properties = displayMode ? { className: ["rich-formula", "rich-formula-block"] } : { className: ["rich-formula"] };
  const fallback: Element = { type: "element", tagName: "span", properties, children: [{ type: "text", value: node.value }] };
  if (node.value.length > 1024) return fallback;
  try {
    // Only KaTeX's generated markup is parsed as HTML.
    const html = katex.renderToString(node.value, {
      displayMode, trust: false, strict: "ignore", maxExpand: 128, maxSize: 20,
    });
    const children = fromHtmlIsomorphic(html, { fragment: true }).children;
    return { ...fallback, children: children.filter(child => child.type === "element") };
  } catch {
    return fallback;
  }
}

function withinBudget(node: Nodes, source: string, remaining: { nodes: number; math: number }, depth = 0): boolean {
  if (--remaining.nodes < 0 || depth > 32) return false;
  if (node.type === "inlineMath" || node.type === "math") {
    remaining.math -= 64 + Math.min(node.value.length, 1024);
    if (remaining.math < 0) return false;
    if (node.type === "inlineMath") {
      const start = node.position?.start.offset ?? -1;
      if (source.startsWith("$$", start) || source.startsWith("\\[", start)) {
        node.data = { hName: "div" };
      }
    }
  }
  // Rows with omitted cells still expand to the full header width.
  if (node.type === "table" && (node.children[0]?.children.length ?? 0) * node.children.length > 1024) return false;
  return !("children" in node) || node.children.every(child => withinBudget(child, source, remaining, depth + 1));
}

function parse(source: string, key?: object): ParsedMessage {
  const cached = key && normalMessages.get(key);
  if (cached?.source === source) return cached;
  const result: ParsedMessage = { source };
  if (source.length <= maxSource && markdownHints.test(source)) {
    try {
      const tree = parser.runSync(parser.parse(source)) as Root;
      if (withinBudget(tree, source, { nodes: maxNodes, math: 4096 })) result.tree = tree;
    } catch {
      // The original text stays readable when a parser cannot handle a message.
    }
  }
  if (key) normalMessages.set(key, result);
  return result;
}

export function renderMessage(source: string, key?: object): HtmlRoot | null {
  const parsed = parse(source, key);
  if (parsed.rendered !== undefined) return parsed.rendered;
  try {
    parsed.rendered = parsed.tree ? renderer.runSync(parsed.tree) : null;
  } catch {
    parsed.rendered = null;
  }
  return parsed.rendered;
}

export function reuseMessageRendering(previous: object, current: object, source: string | null) {
  const parsed = normalMessages.get(previous);
  if (parsed && parsed.source === source) normalMessages.set(current, parsed);
}

function summaryText(node: Nodes): string {
  switch (node.type) {
    case "image":
    case "imageReference":
      return node.alt ?? "";
    case "code":
    case "math":
      return node.value.split(/\r?\n/, 1)[0];
    case "text":
    case "inlineCode":
    case "inlineMath":
    case "html":
      return node.value;
    case "break":
    case "thematicBreak":
      return " ";
    default:
      return "children" in node ? node.children.map(summaryText)
        .join(["paragraph", "heading", "emphasis", "strong", "delete", "link", "linkReference"].includes(node.type) ? "" : " ") : "";
  }
}

export function messageSummary(source: string, key?: object) {
  const parsed = parse(source, key);
  if (parsed.summary !== undefined) return parsed.summary;
  const summary = (parsed.tree ? summaryText(parsed.tree) : source).replace(/\s+/g, " ").trim();
  const chars = [...summary];
  return parsed.summary = chars.length > maxSummary ? chars.slice(0, maxSummary).join("").trimEnd() + "…" : summary;
}

export function conversationSummary(source: string) {
  // The directory sends a 100-character preview, which can end inside an image or link.
  const prefix = [...source].length === 100 ? source.replace(/!?\[([^\]\n]*)\]\([^\n)]*$/, "$1") : source;
  return messageSummary(prefix);
}
