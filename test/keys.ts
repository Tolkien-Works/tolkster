// The keys of T, every one named: a field added to T fails to compile here until the
// fixture has it too.
export function keys<T>(shape: { [K in keyof Required<T>]: true }): string[] {
  return Object.keys(shape).sort();
}
