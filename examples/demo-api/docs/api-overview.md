# API Overview

## Architecture

The application is initialized by `create_app` in `app.py`, which:
- Loads configuration from `Config` classes
- Registers route blueprints for auth, users, and posts
- Sets up middleware via `setup_logging` and `setup_error_handlers`

## Endpoints

### Authentication (`auth_routes.py`)
- `POST /auth/register` - calls `register()` to create new users
- `POST /auth/login` - calls `login()` to authenticate and return JWT
- `GET /auth/me` - calls `get_current_user()` for profile info
- `POST /auth/refresh` - calls `refresh_token()` to renew JWT

### Users (`user_routes.py`)
- `GET /users` - calls `list_users()` with pagination
- `GET /users/:id` - calls `get_user()` for profile
- `PUT /users/:id` - calls `update_user()` to modify profile
- `DELETE /users/:id` - calls `delete_user()` (admin only)

### Posts (`post_routes.py`)
- `GET /posts` - calls `list_posts()` with pagination
- `POST /posts` - calls `create_post()` (authenticated)
- `GET /posts/:id` - calls `get_post()` with comments

## Database Layer

All data access goes through `database.py`:
- `create_user`, `get_user`, `update_user` for user records
- `create_post`, `get_post`, `get_all_posts` for content
- `create_comment`, `get_comment` for discussions
