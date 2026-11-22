import { Store } from "./index";

export function createMemory(seed: string): Store {
  const store = new Store();
  store.load(seed);
  return store;
}
