import { parse as parseList } from "@acme/core";
import { truncate } from "~utils/strings";

export function format(title: string): string {
  return `# ${title}`;
}

export function render(title: string): string {
  return format(truncate(title, 40));
}

export function renderList(raw: string): string[] {
  return parseList(raw).map((item) => render(item));
}
