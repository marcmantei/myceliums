from services.user_service import UserService
from utils.helpers import format_name


def main():
    service = UserService()
    user = service.get_user("123")
    if user:
        name = format_name(user["name"])
        print(name)


def process_request(data: dict) -> dict:
    validated = validate_input(data)
    result = transform_data(validated)
    return result


def validate_input(data: dict) -> dict:
    if not data:
        raise ValueError("Invalid input")
    return data


def transform_data(data: dict) -> dict:
    return {k: str(v).strip() for k, v in data.items()}


if __name__ == "__main__":
    main()
