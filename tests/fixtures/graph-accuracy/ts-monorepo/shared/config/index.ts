export function readFlag(name: string): boolean {
  return name.startsWith("--");
}
