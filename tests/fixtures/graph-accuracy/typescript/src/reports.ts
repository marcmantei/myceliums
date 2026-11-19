import * as formatting from "./formatting";
import { formatName, Order } from "./models";

export interface Sink {
  write(text: string): void;
}

export class ConsoleSink implements Sink {
  write(text: string): void {
    console.log(text);
  }
}

export class MemorySink implements Sink {
  readonly lines: string[] = [];

  write(text: string): void {
    this.lines.push(text);
  }
}

export class Report {
  constructor(protected readonly title: string) {}

  header(): string {
    return formatting.render(this.title);
  }

  render(rows: unknown[]): string {
    const lines = [this.header()];
    for (const row of rows) {
      lines.push(this.renderRow(row));
    }
    return lines.join("\n");
  }

  renderRow(row: unknown): string {
    return String(row);
  }
}

export class OrderReport extends Report {
  constructor(private readonly orders: Order[]) {
    super("Orders");
  }

  renderRow(order: Order): string {
    return `${formatName(order.user.name)}: ${order.total()}`;
  }

  summary(): string {
    return this.render(this.orders);
  }

  static empty(): string {
    return new OrderReport([]).summary();
  }
}

export function publish(report: Report, sink: Sink): void {
  sink.write(report.render([]));
}
