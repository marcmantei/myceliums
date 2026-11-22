import { debounce } from "lodash";
import * as core from "@acme/core";
import { Counter, format as formatText } from "@acme/core";
import { Store } from "@acme/store";
import { createMemory } from "@acme/store/memory";
import { readFlag } from "@shared/config";
import { slugify } from "~utils/strings";
import { render } from "./render";

export function start(argv: string[]): string {
  const store = new Store();
  store.load(argv.join(","));
  const memory = createMemory("a,b");
  const title = slugify(formatText(store.all()));
  const counter = new Counter();
  counter.add(core.sum([memory.all().length, 1]));
  if (readFlag(argv[0])) {
    store.clear();
  }
  const later = debounce(() => render(title), 10);
  later();
  return render(title);
}
