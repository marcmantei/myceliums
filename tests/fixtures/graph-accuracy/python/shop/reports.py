"""Plain-text reports."""

from shop import formatting
from shop.models import Order, format_name


class Report:
    def __init__(self, title):
        self.title = title

    def header(self):
        return formatting.render(self.title)

    def render(self, rows):
        lines = [self.header()]
        lines.extend(self.render_row(row) for row in rows)
        return "\n".join(lines)

    def render_row(self, row):
        return str(row)


class OrderReport(Report):
    def __init__(self, orders):
        super().__init__("Orders")
        self.orders = orders

    def render_row(self, order: Order):
        return f"{format_name(order.user.name)}: {order.total()}"

    def summary(self):
        return self.render(self.orders)

    @staticmethod
    def empty():
        return OrderReport([]).summary()
