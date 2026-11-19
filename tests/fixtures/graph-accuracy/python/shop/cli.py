"""Command-line entry point."""

from shop.config import Config
from shop.models import format_name
from shop.service import OrderService, UserService
from shop.storage import Repository


def handler(event):
    return {"handled": event}


def dispatch(event):
    handler = lambda queued: {"queued": queued}
    return handler(event)


def main(argv):
    repo = Repository()
    users = UserService(repo)
    orders = OrderService(repo)
    user = users.register(argv[0], argv[1])
    orders.place(user, argv[2:])
    print(format_name(user.name))
    dispatch(argv)
    handler(argv)
    return Config({"argv": argv}).get("argv")
