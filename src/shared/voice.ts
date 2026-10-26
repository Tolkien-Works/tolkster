// Tolkster's in-progress phrases, one list per state, from voice.json. The CLI reads the
// same file (cli/src/voice.rs), so the Worker and the workers speak alike. How they
// should sound is in voice.md, beside them.
//
// A phrase is fixed text from the file: never build one from issue, comment or review
// text. Nothing reads them back, so they can change freely.
import voice from "./voice.json" with { type: "json" };

export type State = keyof typeof voice.states;

export function phrases(state: State): readonly string[] {
  return voice.states[state].phrases;
}

// One of the state's phrases, at random, so the same step doesn't read the same every
// time. `random` is Math.random's shape, so a test can fix the pick.
export function phrase(state: State, random: () => number = Math.random): string {
  const list = phrases(state);
  return list[Math.min(list.length - 1, Math.floor(random() * list.length))]!;
}
