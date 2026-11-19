import { Repository } from "./storage";

export interface Item {
  price: number;
}

export const formatName = (name: string): string => name.trim().toUpperCase();

export class User {
  constructor(
    public readonly name: string,
    public readonly email: string,
  ) {}

  save(repo: Repository<User>): User {
    repo.add(this);
    return this;
  }

  displayName(): string {
    return formatName(this.name);
  }
}

export class Order {
  constructor(
    public readonly user: User,
    private readonly items: Item[],
  ) {}

  save(repo: Repository<Order>): Order {
    repo.add(this);
    return this;
  }

  total(): number {
    return this.items.map((item) => item.price).reduce((a, b) => a + b, 0);
  }

  add(item: Item): void {
    this.items.push(item);
  }
}
