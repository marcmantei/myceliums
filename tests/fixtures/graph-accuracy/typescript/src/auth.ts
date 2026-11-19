import { validate } from "./validation";

export interface Token {
  user: string;
  scope: string;
  expired?: boolean;
}

export function parse(raw: string): Token {
  const parts = raw.split(".");
  if (!validate(parts)) {
    throw new Error("bad token");
  }
  return decode(parts);
}

export function decode(parts: string[]): Token {
  return { user: parts[0], scope: parts[1] };
}

export function refresh(token: Token, attempts = 0): Token | null {
  if (attempts > 3) {
    return null;
  }
  if (token.expired) {
    return refresh({ ...token, expired: false }, attempts + 1);
  }
  return token;
}
