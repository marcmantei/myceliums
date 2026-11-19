package shop.auth;

/** A decoded bearer token. */
public final class Token {
    public final String user;
    public final String scope;
    public final boolean expired;

    public Token(String user, String scope, boolean expired) {
        this.user = user;
        this.scope = scope;
        this.expired = expired;
    }
}
