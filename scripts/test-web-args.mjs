// Checks how the web page turns a command into arguments (`cliArgs` in app.js): values that
// start with `-` must reach clap as values, never as flags.
//   node scripts/test-web-args.mjs
import { readFileSync } from "node:fs";
import assert from "node:assert/strict";

const src = readFileSync(new URL("../src/interfaces/web/assets/app.js", import.meta.url), "utf8");
const fn = src.match(/^function cliArgs\(args\) \{\n[\s\S]*?\n\}\n/m);
assert.ok(fn, "app.js has no top-level function cliArgs");
const cliArgs = new Function(`${fn[0]}; return cliArgs;`)();

const cases = [
  [["list"], ["list"]],
  [["save", "work"], ["save", "work"]],
  [["save", "-x"], ["save", "--", "-x"]],
  [["delete", "-x", "--yes"], ["delete", "--yes", "--", "-x"]],
  [["show", "-x@2"], ["show", "--", "-x@2"]],
  [["show", "--x"], ["show", "--", "--x"]],
  [["diff", "-x@1", "-x@2"], ["diff", "--", "-x@1", "-x@2"]],
  [["edit", "-x", "--batch", "-", "-m", "- fixed"], ["edit", "--batch", "-", "--message=- fixed", "--", "-x"]],
  [["edit", "work", "--batch", "-", "--base", "3"], ["edit", "--batch", "-", "--base=3", "work"]],
  [["sessions", "--search", "-foo", "--days", "365"], ["sessions", "--search=-foo", "--days=365"]],
  [["restore", "-x", "--plan", "--tab", "-w#1"], ["restore", "--plan", "--tab=-w#1", "--", "-x"]],
  [["apply", "p1"], ["apply", "p1"]],
];
for (const [input, want] of cases) {
  assert.deepEqual(cliArgs(input), want, JSON.stringify(input));
}
console.log(`web page arguments: ${cases.length} cases ok`);
