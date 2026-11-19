import { parse as parseToken, refresh, Token } from "./auth";
import * as config from "./config";
import { Item, Order, User } from "./models";
import { Repository } from "./storage";
import { map } from "./utils";
import { isEmail, Validator } from "./validation";

export class UserService {
  private readonly validator: Validator;

  constructor(private readonly repo: Repository<User>) {
    this.validator = new Validator([isEmail]);
  }

  register(name: string, email: string): User {
    if (!this.validator.validate(email)) {
      throw new Error("invalid email");
    }
    const user = new User(name, email);
    return user.save(this.repo);
  }

  login(raw: string): Token | null {
    const token = parseToken(raw);
    return refresh(token);
  }

  settings(text: string): string {
    const cfg = config.Config.load(text);
    return cfg.get("theme") ?? "light";
  }

  get(index: number): User {
    return this.repo.get(index);
  }

  names(): string[] {
    return map(this.repo.all(), (user) => user.name);
  }
}

export class OrderService {
  constructor(private readonly repo: Repository<Order>) {}

  place(user: User, items: Item[]): number {
    const order = new Order(user, items);
    order.save(this.repo);
    return order.total();
  }

  parse(text: string): string {
    const values = config.parse(text);
    return values.get("currency") ?? "EUR";
  }
}
