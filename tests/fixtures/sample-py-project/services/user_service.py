from typing import Optional


class UserService:
    def __init__(self):
        self.users = {}

    def get_user(self, user_id: str) -> Optional[dict]:
        return self.users.get(user_id)

    def create_user(self, user_id: str, name: str, email: str) -> dict:
        user = {"id": user_id, "name": name, "email": email}
        self.users[user_id] = user
        return user

    def delete_user(self, user_id: str) -> bool:
        if user_id in self.users:
            del self.users[user_id]
            return True
        return False

    def list_users(self) -> list:
        return list(self.users.values())
