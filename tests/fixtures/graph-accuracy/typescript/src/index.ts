import { Config } from "./config";
import { formatName, Item, Order, User } from "./models";
import { ConsoleSink, OrderReport, publish } from "./reports";
import { OrderService, UserService } from "./service";
import { Repository } from "./storage";

export function handler(event: string[]): { handled: string[] } {
  return { handled: event };
}

export function dispatch(event: string[]): { queued: string[] } {
  const handler = (queued: string[]) => ({ queued });
  return handler(event);
}

export function main(argv: string[], items: Item[]): string | undefined {
  const users = new UserService(new Repository<User>());
  const orders = new OrderService(new Repository<Order>());
  const user = users.register(argv[0], argv[1]);
  orders.place(user, items);
  console.log(formatName(user.name));
  dispatch(argv);
  handler(argv);
  publish(new OrderReport([]), new ConsoleSink());
  return new Config(new Map([["argv", argv.join(" ")]])).get("argv");
}
