import { Counter, parse } from "@acme/core";

export class Store {
  private readonly items: string[] = [];
  private readonly counter: Counter;

  constructor() {
    this.counter = new Counter();
  }

  load(raw: string): number {
    for (const item of parse(raw)) {
      this.items.push(item);
      this.counter.add(1);
    }
    return this.items.length;
  }

  all(): string[] {
    return [...this.items];
  }

  clear(): void {
    this.items.length = 0;
    this.counter.reset();
  }
}
