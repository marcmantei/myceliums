export function parse(text: string): Map<string, string> {
  const data = JSON.parse(text) as Record<string, string>;
  return normalize(data);
}

export function normalize(data: Record<string, string>): Map<string, string> {
  const values = new Map<string, string>();
  for (const [key, value] of Object.entries(data)) {
    values.set(key.toLowerCase(), value);
  }
  return values;
}

export class Config {
  constructor(private readonly values: Map<string, string>) {}

  static load(text: string): Config {
    return new Config(parse(text));
  }

  get(key: string): string | undefined {
    return this.values.get(key);
  }
}
