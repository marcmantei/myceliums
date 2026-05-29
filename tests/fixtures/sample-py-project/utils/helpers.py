def format_name(name: str) -> str:
    return " ".join(capitalize(word) for word in name.strip().split())


def capitalize(word: str) -> str:
    return word[0].upper() + word[1:].lower() if word else ""
