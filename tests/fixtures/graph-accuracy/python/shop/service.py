"""Application services."""

from shop import config
from shop.auth import parse as parse_token
from shop.auth import refresh
from shop.models import Order, User
from shop.storage import Repository
from shop.validation import Validator, is_email


class UserService:
    def __init__(self, repo: Repository):
        self.repo = repo
        self.validator = Validator([is_email])

    def register(self, name, email):
        if not self.validator.validate(email):
            raise ValueError("invalid email")
        user = User(name, email)
        return user.save(self.repo)

    def login(self, raw_token):
        token = parse_token(raw_token)
        return refresh(token)

    def settings(self, text):
        cfg = config.Config.load(text)
        return cfg.get("theme", "light")

    def get(self, index):
        return self.repo.get(index)


class OrderService:
    def __init__(self, repo: Repository):
        self.repo = repo

    def place(self, user: User, items):
        order = Order(user, items)
        order.save(self.repo)
        return order.total()

    def parse(self, text):
        values = config.parse(text)
        return values.get("currency", "EUR")
