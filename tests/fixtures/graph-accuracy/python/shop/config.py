"""Configuration loading."""

import json


def parse(text):
    """Parse a JSON configuration document."""
    data = json.loads(text)
    return normalize(data)


def normalize(data):
    return {key.lower(): value for key, value in data.items()}


class Config:
    def __init__(self, values):
        self.values = values

    @classmethod
    def load(cls, text):
        return cls(parse(text))

    def get(self, key, default=None):
        return self.values.get(key, default)
