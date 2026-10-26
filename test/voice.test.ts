import assert from "node:assert/strict";
import { test } from "node:test";
import voice from "../src/shared/voice.json" with { type: "json" };
import { phrase, phrases, type State } from "../src/shared/voice.ts";

const states = Object.keys(voice.states) as State[];

test("every state has distinct, trimmed phrases", () => {
  assert.ok(states.length > 0);
  for (const state of states) {
    const list = phrases(state);
    assert.ok(list.length > 0, `${state} has no phrases`);
    assert.equal(new Set(list).size, list.length, `${state} repeats a phrase`);
    for (const p of list) assert.ok(p.length > 0 && p.trim() === p, `${state}: ${JSON.stringify(p)}`);
  }
});

test("a phrase comes from its state's list, whatever the random number", () => {
  for (const state of states) {
    const list = phrases(state);
    assert.equal(phrase(state, () => 0), list[0]);
    assert.equal(phrase(state, () => 0.999999), list.at(-1));
    // Math.random never returns 1, but a bad stand-in mustn't read past the end.
    assert.equal(phrase(state, () => 1), list.at(-1));
    for (let i = 0; i < 50; i++) assert.ok(list.includes(phrase(state)));
  }
});

test("phrases never name the machinery", () => {
  for (const state of states) {
    for (const p of phrases(state)) assert.doesNotMatch(p, /workstation|orca|worktree|terminal|the worker/i, `${state}: ${p}`);
  }
});
