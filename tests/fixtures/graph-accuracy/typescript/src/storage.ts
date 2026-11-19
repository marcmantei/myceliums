export class Repository<T> {
  private readonly rows: T[] = [];

  add(row: T): number {
    this.rows.push(row);
    return this.rows.length;
  }

  get(index: number): T {
    return this.rows[index];
  }

  find(predicate: (row: T) => boolean): T | undefined {
    return this.rows.find(predicate);
  }

  all(): T[] {
    return [...this.rows];
  }
}
