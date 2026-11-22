export function parse(raw: string): string[] {
  return raw.split(",").map((part) => normalize(part));
}

export function normalize(part: string): string {
  return part.trim().toLowerCase();
}

export function format(values: string[]): string {
  return values.map((value) => normalize(value)).join(", ");
}
