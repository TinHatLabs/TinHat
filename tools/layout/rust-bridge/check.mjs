#!/usr/bin/env node
// check.mjs — the independent invariant checker (../invariants.mjs, the contract's byte copy) as a line filter for the
// Rust port's tests (crates/layout/tests): one JSON request per stdin line, one JSON answer line each (the list of
// violations; [] = pass).
//   {"kind":"solve","input":{form,size,unitPm,shape,content,view},"out":…,"out2":…}  → checkSolve(input, out, out2)
//   {"kind":"op","before":form,"op":op,"res":{ok,form?,error?}}                       → checkOpResult(before, op, res)
// The schema: $LAYOUT_SCHEMA, else ../../crates/layout/layout.schema.json (invariants.mjs's own lookup).
import readline from "node:readline";
import { checkOpResult, checkSolve } from "../invariants.mjs";

const rl = readline.createInterface({ input: process.stdin, crlfDelay: Infinity });
for await (const line of rl) {
  if (!line) continue;
  let errs;
  try {
    const r = JSON.parse(line);
    if (r.kind === "solve") errs = checkSolve(r.input, r.out, r.out2);
    else if (r.kind === "op") errs = checkOpResult(r.before, r.op, r.res);
    else errs = [`check.mjs: unknown request kind ${r.kind}`];
  } catch (e) {
    errs = [`check.mjs: ${e}`];
  }
  process.stdout.write(JSON.stringify(errs) + "\n");
}
