export function map<T, U>(items: T[], f: (item: T) => U): U[] {
  const out: U[] = [];
  for (const item of items) {
    out.push(f(item));
  }
  return out;
}
