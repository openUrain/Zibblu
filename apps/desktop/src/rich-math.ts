import type { Extension as MarkdownExtension } from "mdast-util-from-markdown";
import { asciiAlpha, asciiDigit, markdownLineEnding, markdownSpace } from "micromark-util-character";
import { codes } from "micromark-util-symbol";
import type { Construct, State } from "micromark-util-types";
import type { Plugin } from "unified";

declare module "micromark-util-types" {
  interface TokenTypeMap {
    messageMath: "messageMath";
    messageMathClose: "messageMathClose";
    messageMathLineEnding: "messageMathLineEnding";
  }
}

const math: Construct = {
  tokenize(effects, ok, nok) {
    let end: number;
    let count = 0;
    let previous: number | null = null;
    let numericStart = false;
    return start;

    function start(code: number | null): State | undefined {
      effects.enter("messageMath");
      effects.consume(code!);
      if (code === codes.dollarSign) {
        end = codes.dollarSign;
        return firstDollarContent;
      }
      return bracket;
    }

    function bracket(code: number | null) {
      if (code !== codes.leftParenthesis && code !== codes.leftSquareBracket) return nok(code);
      end = code === codes.leftParenthesis ? codes.rightParenthesis : codes.rightSquareBracket;
      effects.consume(code);
      return content;
    }

    function firstDollarContent(code: number | null) {
      if (code === codes.dollarSign || markdownSpace(code)) return nok(code);
      numericStart = asciiDigit(code);
      return content(code);
    }

    function content(code: number | null): ReturnType<State> {
      if (code === codes.eof || count >= 1024 || (end === codes.dollarSign && code === codes.graveAccent)
        || (end !== codes.rightSquareBracket && markdownLineEnding(code))) return nok(code);
      if (markdownLineEnding(code)) {
        effects.enter("messageMathLineEnding");
        effects.consume(code);
        effects.exit("messageMathLineEnding");
        count++;
        return content;
      }
      if (code === codes.backslash) {
        effects.consume(code);
        count++;
        return escaped;
      }
      if (code === codes.dollarSign && end === codes.dollarSign) {
        if (count === 0 || markdownSpace(previous)) return nok(code);
        effects.consume(code);
        return afterDollar;
      }
      return consume(code);
    }

    function escaped(code: number | null) {
      if (code === end && end !== codes.dollarSign) {
        if (count === 1) return nok(code);
        // Micromark needs a token on the closing line even if it only contains the fence.
        effects.enter("messageMathClose");
        effects.consume(code);
        effects.exit("messageMathClose");
        effects.exit("messageMath");
        return ok;
      }
      if (code === codes.backslash || (end === codes.dollarSign && (code === codes.dollarSign || code === codes.graveAccent))) {
        return consume(code);
      }
      if ((end === codes.rightParenthesis && code === codes.leftParenthesis)
        || (end === codes.rightSquareBracket && code === codes.leftSquareBracket)) return nok(code);
      return content(code);
    }

    function afterDollar(code: number | null) {
      // Leave the next price or formula opener available to the Markdown parser.
      if (code === codes.dollarSign || asciiDigit(code) || (numericStart && asciiAlpha(code))) return nok(code);
      effects.exit("messageMath");
      return ok(code);
    }

    function consume(code: number | null) {
      effects.consume(code!);
      previous = code;
      count++;
      return content;
    }
  },
};

const markdown: MarkdownExtension = {
  enter: {
    messageMath(token) {
      const raw = this.sliceSerialize(token);
      const delimiter = raw.startsWith("$") ? 1 : 2;
      const value = raw.slice(delimiter, -delimiter).trim();
      this.enter(value ? {
        type: "inlineMath", value,
      } : { type: "text", value: raw }, token);
    },
  },
  exit: { messageMath(token) { this.exit(token); } },
};

export const messageMath: Plugin = function () {
  const data = this.data();
  (data.micromarkExtensions ??= []).push({
    text: { [codes.backslash]: math, [codes.dollarSign]: { ...math, previous: code => code !== codes.dollarSign } },
  });
  (data.fromMarkdownExtensions ??= []).push(markdown);
};
