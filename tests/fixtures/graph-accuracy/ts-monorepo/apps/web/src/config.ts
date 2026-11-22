import { parse } from "yaml";

export function loadConfig(text: string): unknown {
  return parse(text);
}
