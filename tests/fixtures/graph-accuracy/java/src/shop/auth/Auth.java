package shop.auth;

import shop.validation.Validation;

/** Bearer-token handling. */
public final class Auth {
    private Auth() {}

    public static Token parse(String raw) {
        String[] parts = raw.split("\\.");
        if (!Validation.validate(parts)) {
            throw new IllegalArgumentException("bad token");
        }
        return decode(parts);
    }

    static Token decode(String[] parts) {
        return new Token(parts[0], parts[1], false);
    }

    public static Token refresh(Token token, int attempts) {
        if (attempts > 3) {
            return null;
        }
        if (token.expired) {
            return refresh(new Token(token.user, token.scope, false), attempts + 1);
        }
        return token;
    }
}
