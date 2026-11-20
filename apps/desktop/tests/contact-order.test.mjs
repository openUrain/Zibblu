import assert from "node:assert/strict";
import test from "node:test";
import { orderContacts } from "../src/contact-order.ts";

test("sorts mixed contacts by pinyin and Latin letters with other initials last", () => {
  const users = [
    { id: "zhao", handle: "friend1" },
    { id: "ten", handle: "10Pages" },
    { id: "wang", handle: "friend2" },
    { id: "apple", handle: "apple" },
    { id: "symbols", handle: "symbols" },
    { id: "a", handle: "a" },
    { id: "two", handle: "2Pages" },
    { id: "bai", handle: "friend3" },
    { id: "a2", handle: "a2" },
    { id: "accent", handle: "accent" },
  ];
  const names = {
    zhao: "赵敏", wang: "王明", bai: "白桦", a: "阿明", a2: "阿明", symbols: "🌿绿茶", accent: "Élodie",
    apple: "  ",
  };
  const expected = [
    ["a", "阿明", "A", true], ["a2", "阿明", "A", true], ["apple", "apple", "A", false],
    ["bai", "白桦", "B", false], ["accent", "Élodie", "E", false], ["wang", "王明", "W", false],
    ["zhao", "赵敏", "Z", false], ["symbols", "🌿绿茶", "#", false],
    ["two", "2Pages", "#", false], ["ten", "10Pages", "#", false],
  ];
  for (const input of [users, users.toReversed()]) {
    assert.deepEqual(orderContacts(input, names).map(({ user, name, letter, showHandle }) => [user.id, name, letter, showHandle]), expected);
  }
});
