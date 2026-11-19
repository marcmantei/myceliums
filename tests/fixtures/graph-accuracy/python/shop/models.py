"""Domain objects."""

from shop.storage import Repository


class User:
    def __init__(self, name, email):
        self.name = name
        self.email = email

    def save(self, repo: Repository):
        repo.add(self)
        return self

    def display_name(self):
        return format_name(self.name)


class Order:
    def __init__(self, user: User, items):
        self.user = user
        self.items = list(items)

    def save(self, repo: Repository):
        repo.add(self)
        return self

    def total(self):
        return sum(item.price for item in self.items)

    def add(self, item):
        self.items.append(item)


def format_name(name):
    return name.strip().title()
