"""Bearer-token handling."""

from shop.validation import validate


def parse(raw):
    """Split a bearer token into its parts and check them."""
    parts = raw.split(".")
    if not validate(parts):
        raise ValueError("bad token")
    return decode(parts)


def decode(parts):
    return {"user": parts[0], "scope": parts[1]}


def refresh(token, attempts=0):
    if attempts > 3:
        return None
    if token.get("expired"):
        return refresh(dict(token, expired=False), attempts + 1)
    return token
