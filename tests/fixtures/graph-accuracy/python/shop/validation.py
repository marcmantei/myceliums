"""Input validation."""


def validate(parts):
    return len(parts) == 2 and all(parts)


def is_email(value):
    return "@" in value


class Validator:
    def __init__(self, rules):
        self.rules = rules

    def validate(self, value):
        return all(self.check(rule, value) for rule in self.rules)

    def check(self, rule, value):
        return rule(value)
