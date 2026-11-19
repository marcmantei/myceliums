export type Rule = (value: string) => boolean;

export function validate(parts: string[]): boolean {
  return parts.length === 2 && parts.every((part) => part.length > 0);
}

export function isEmail(value: string): boolean {
  return value.includes("@");
}

export class Validator {
  constructor(private readonly rules: Rule[]) {}

  validate(value: string): boolean {
    return this.rules.every((rule) => this.check(rule, value));
  }

  check(rule: Rule, value: string): boolean {
    return rule(value);
  }
}
