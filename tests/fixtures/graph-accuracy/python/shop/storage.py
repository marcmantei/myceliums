"""In-memory persistence."""


class Repository:
    def __init__(self):
        self._rows = []

    def add(self, row):
        self._rows.append(row)
        return len(self._rows)

    def get(self, index):
        return self._rows[index]

    def find(self, predicate):
        for row in self._rows:
            if predicate(row):
                return row
        return None
