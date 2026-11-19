"""Text formatting helpers."""


def render(text):
    return text.upper()


def pad(text, width):
    return render(text).ljust(width)
