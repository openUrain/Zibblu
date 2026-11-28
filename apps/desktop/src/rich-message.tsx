import type { Element, ElementContent } from "hast";
import { toJsxRuntime } from "hast-util-to-jsx-runtime";
import bash from "highlight.js/lib/languages/bash";
import javascript from "highlight.js/lib/languages/javascript";
import json from "highlight.js/lib/languages/json";
import python from "highlight.js/lib/languages/python";
import rust from "highlight.js/lib/languages/rust";
import typescript from "highlight.js/lib/languages/typescript";
import { Check, CircleAlert, Copy } from "lucide-react";
import { createLowlight } from "lowlight";
import { memo, useContext, useMemo, useState } from "react";
import { Fragment, jsx, jsxs } from "react/jsx-runtime";
import type { Components } from "hast-util-to-jsx-runtime";
import "katex/dist/katex.min.css";
import { copyText } from "./context-menu";
import { LanguageContext, t } from "./locale";
import { RichLink } from "./rich-links";
import { renderMessage } from "./rich-markdown";
import "./rich-message.css";

const highlighter = createLowlight({ bash, javascript, json, python, rust, typescript });
highlighter.registerAlias({ bash: ["sh", "shell"], typescript: ["ts"], javascript: ["js"], python: ["py"], rust: ["rs"] });
const runtime = { Fragment, jsx, jsxs };

function codeContent(node?: Element) {
  const code = node?.children[0];
  if (code?.type !== "element" || code.tagName !== "code") return;
  const language = Array.isArray(code.properties.className)
    ? code.properties.className.find(value => typeof value === "string" && value.startsWith("language-")) : undefined;
  return {
    language: typeof language === "string" ? language.slice(9, 9 + 32) : "",
    text: code.children.map(child => child.type === "text" ? child.value : "").join("").replace(/\n$/, ""),
  };
}

function CodeBlock({ language, text }: { language: string; text: string }) {
  const [status, setStatus] = useState<"copied" | "failed">();
  const label = t("Copy code", "复制代码");
  const feedback = status === "copied" ? t("Copied", "已复制") : status === "failed" ? t("Copy failed", "复制失败") : "";
  const Icon = status === "copied" ? Check : status === "failed" ? CircleAlert : Copy;
  const highlighted = useMemo(() => {
    try {
      const name = language.toLowerCase();
      if (text.length <= 8192 && highlighter.registered(name)) return toJsxRuntime(highlighter.highlight(name, text), runtime);
    } catch {
      // A grammar may reject input. The code and copy button still work.
    }
    return text;
  }, [language, text]);
  return <div className="rich-code">
    <div className="rich-code-actions">
      <span>{language || t("Code", "代码")}</span>
      <button type="button" className="icon-button" aria-label={label} title={feedback || label} onClick={() => {
        setStatus(undefined);
        void copyText(text).then(() => setStatus("copied")).catch(() => setStatus("failed"));
      }}><Icon size={14} aria-hidden="true" /></button>
    </div>
    <pre className="rich-scroll" tabIndex={0} aria-label={t("Code", "代码")}><code>{highlighted}</code></pre>
    <span className="sr-only" role="status">{feedback}</span>
  </div>;
}

function containsImage(children: ElementContent[]): boolean {
  return children.some(child => child.type === "element" && (child.tagName === "img" || containsImage(child.children)));
}

function components(privateRoom: boolean): Partial<Components> {
  return {
    a: ({ node, href, children }) => node && containsImage(node.children) ? <>{children}</>
      : <RichLink source={href ?? ""} privateRoom={privateRoom}>{children}</RichLink>,
    img: ({ src, alt }) => <RichLink source={src ?? ""} image privateRoom={privateRoom}>
      {alt?.trim() || t("Image", "图片")}
    </RichLink>,
    pre: ({ node, children }) => {
      const code = codeContent(node);
      return code ? <CodeBlock {...code} /> : <pre>{children}</pre>;
    },
    table: ({ node: _node, ...props }) => <div className="rich-scroll" tabIndex={0} role="group" aria-label={t("Table", "表格")}>
      <table {...props} />
    </div>,
    span: ({ node: _node, ...props }) => props.className?.includes("rich-formula-block")
      ? <span {...props} tabIndex={0} role="group" aria-label={t("Formula", "公式")} />
      : <span {...props} />,
  };
}

const normalComponents = components(false);
const privateComponents = components(true);

export const RichMessage = memo(function RichMessage({ text, cacheKey, privateRoom = false }: {
  text?: string | null; cacheKey?: object; privateRoom?: boolean;
}) {
  const language = useContext(LanguageContext);
  const tree = useMemo(() => text ? renderMessage(text, privateRoom ? undefined : cacheKey) : null, [text, cacheKey, privateRoom]);
  const body = useMemo(() => tree ? toJsxRuntime(tree, {
    ...runtime, components: privateRoom ? privateComponents : normalComponents, passNode: true, elementAttributeNameCase: "react",
  }) : null, [tree, privateRoom, language]);
  return <div className={tree ? "bubble rich-message rich-markdown" : "bubble rich-message"}>{body ?? text}</div>;
});
