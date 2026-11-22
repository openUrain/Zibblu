import assert from "node:assert/strict";
import { after, test } from "node:test";
import { createServer } from "vite";

const vite = await createServer({ configFile: false, server: { watch: null }, appType: "custom" });
after(() => vite.close());
const { renderMessage, messageSummary, conversationSummary, safeWebUrl } = await vite.ssrLoadModule("/src/rich-markdown.ts");
const { ChatHistoryCache } = await vite.ssrLoadModule("/src/chat-history-cache.ts");

function walk(node, fn) {
  if (node) {
    fn(node);
    for (const child of node.children ?? []) walk(child, fn);
  }
}

function elements(tree, tag) {
  const found = [];
  walk(tree, node => { if (node.tagName === tag) found.push(node); });
  return found;
}

function text(tree) {
  const parts = [];
  walk(tree, node => { if (node.type === "text") parts.push(node.value); });
  return parts.join("");
}

test("math delimiters keep prices, escapes and code literal", () => {
  const source = [
    "价格 $5，我花了 $10；$8,$12；\\$3；`$y$`；`\\(z\\)`；\\\\(escaped\\\\)",
    "Inline $E=mc^2$ and \\(a^2+b^2=c^2\\). Price $5 and $x$.",
    "",
    "$$x^2$$",
    "",
    "$$",
    "y^2",
    "$$",
    "",
    "\\[",
    "\\frac{1}{2}",
    "\\]",
    "",
    "```math",
    "$x$ \\(a\\)",
    "```",
  ].join("\n");
  const tree = renderMessage(source);
  assert.equal(elements(tree, "math").length, 6);
  assert.equal(elements(tree, "span").filter(node => node.properties.className?.includes("rich-formula-block")).length, 3);
  assert.match(text(tree), /价格 \$5，我花了 \$10；\$8,\$12；\$3；\$y\$/);
  assert.ok(text(tree).includes("\\(escaped\\)"));
  assert.match(text(elements(tree, "pre")[0]), /\$x\$ \\\(a\\\)/);
});

test("GFM keeps line breaks, allows read-only tasks and never interprets raw HTML", () => {
  const tree = renderMessage([
    "one", "two", "", "# Heading", "", "- [x] done", "- [ ] next",
    "", "| A | B |", "| --- | --- |", "| **one** | ~~two~~ |", "",
    "<script>globalThis.unsafe = true</script>", "", "broken $\\notACommand$ still here",
    "Blocked $\\includegraphics{https://example.com/image.png}$",
  ].join("\n"));
  assert.ok(elements(tree, "br").length >= 1);
  assert.equal(elements(tree, "table").length, 1);
  assert.equal(elements(tree, "strong").length, 1);
  assert.equal(elements(tree, "del").length, 1);
  assert.deepEqual(elements(tree, "input").map(node => [node.properties.disabled, node.properties.checked]), [[true, true], [true, false]]);
  assert.equal(elements(tree, "script").length, 0);
  assert.equal(elements(tree, "img").length, 0);
  assert.ok(text(tree).includes("<script>globalThis.unsafe = true</script>"));
  assert.ok(text(tree).includes("broken \\notACommand still here"));
});

test("summaries use source text without rendered content or truncated destinations", () => {
  assert.equal(messageSummary("# Heading\n\n**A** ![diagram](https://example.com/p.png) $x^2$"), "Heading A diagram x^2");
  assert.equal(conversationSummary("![diagram](https://example.com/" + "x".repeat(69)), "diagram");
  assert.equal(messageSummary("界".repeat(145)), "界".repeat(140) + "…");
  for (const unsafe of ["javascript:alert(1)", "file:///test", "data:image/png;base64,AB", "//example.com/a", "HTTPS://user@example.com/"]) {
    assert.equal(safeWebUrl(unsafe), undefined);
  }
  assert.equal(safeWebUrl("https://example.com/path?q=a#b"), "https://example.com/path?q=a#b");
});

test("normal rendering is cached by record and content; complex inputs fall back locally", () => {
  const record = {};
  assert.strictEqual(renderMessage("**v1**", record), renderMessage("**v1**", record));
  assert.notStrictEqual(renderMessage("**v1**", record), renderMessage("**v2**", record));
  assert.notStrictEqual(renderMessage("**private**"), renderMessage("**private**"));
  assert.equal(renderMessage("plain\ntext"), null);
  assert.equal(renderMessage("> ".repeat(48) + "nested"), null);
  assert.equal(renderMessage("|" + " h |".repeat(48) + "\n|" + " --- |".repeat(48) + "\n" + "| x |\n".repeat(40)), null);
});

test("history refresh reuses unchanged rendering by conversation, sequence and source", () => {
  const history = new ChatHistoryCache();
  const previous = { seq: 1, text: "**source**" };
  const refreshed = { ...previous };
  history.save("first", { items: [previous], has_more: false });
  const tree = renderMessage(previous.text, previous);
  history.save("first", { items: [refreshed], has_more: false });
  assert.strictEqual(renderMessage(refreshed.text, refreshed), tree);
  const updated = { ...refreshed, text: "*edited*" };
  history.save("first", { items: [updated], has_more: false });
  assert.notStrictEqual(renderMessage(updated.text, updated), tree);
  const other = { ...refreshed };
  history.save("second", { items: [other], has_more: false });
  assert.notStrictEqual(renderMessage(other.text, other), tree);
});
