export function sum(values: number[]): number {
  let total = 0;
  for (const value of values) total += value;
  return total;
}

export class Counter {
  private count = 0;

  add(step: number): number {
    this.count = sum([this.count, step]);
    return this.count;
  }

  reset(): void {
    this.count = 0;
  }
}
