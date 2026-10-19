// Checks how the web page sends commands (app.js): `cliArgs` turns a command into arguments, and
// values that start with `-` must reach clap as values, never as flags; `serializer` sends them
// one at a time.
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

// `serializer` in app.js: the page's commands run one after another, even when one fails.
const ser = src.match(/^function serializer\(\) \{\n[\s\S]*?\n\}\n/m);
assert.ok(ser, "app.js has no top-level function serializer");
const inTurn = new Function(`${ser[0]}; return serializer();`)();
const log = [];
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const task = (name, ms, fail) => async () => {
  log.push(`start ${name}`);
  await sleep(ms);
  log.push(`end ${name}`);
  if (fail) throw new Error(name);
  return name;
};
const results = await Promise.allSettled([inTurn(task("a", 30)), inTurn(task("b", 5, true)), inTurn(task("c", 1))]);
assert.deepEqual(log, ["start a", "end a", "start b", "end b", "start c", "end c"]);
assert.deepEqual(results.map((r) => r.status), ["fulfilled", "rejected", "fulfilled"]);
assert.equal(results[2].value, "c");
console.log("web page commands: one at a time");

// `listable` in app.js: snapshots named `--x` (made before such names were refused) are left out
// of the page's list, since the server won't pass that name.
const lst = src.match(/^function listable\(snapshots\) \{\n[\s\S]*?\n\}\n/m);
assert.ok(lst, "app.js has no top-level function listable");
const listable = new Function(`${lst[0]}; return listable;`)();
assert.deepEqual(listable([{ name: "work" }, { name: "--x" }, { name: "-y" }]),
  { shown: [{ name: "work" }, { name: "-y" }], hidden: 1 });
assert.deepEqual(listable([]), { shown: [], hidden: 0 });
console.log("web page list: leaves out names starting with --");

// A task whose `wanted()` is false by its turn (a search a newer one replaced) never runs.
{
  const inTurn2 = new Function(`${ser[0]}; return serializer();`)();
  const ran = [];
  let latest = 0;
  const search = (q) => {
    const mine = ++latest;
    return inTurn2(async () => { ran.push(q); await sleep(5); return q; }, () => mine === latest);
  };
  const first = inTurn2(async () => { await sleep(20); return "save"; });
  const results = await Promise.all([first, search("a"), search("au"), search("auth")]);
  assert.deepEqual(ran, ["auth"]);
  assert.deepEqual(results, ["save", undefined, undefined, "auth"]);
  console.log("web page commands: a replaced search is skipped");
}
