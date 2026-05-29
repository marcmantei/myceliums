# Demo API - Myceliums Impact Analysis Example

This is a realistic REST API codebase designed to demonstrate Myceliums' impact analysis capabilities. It shows how changing core authentication functions creates a wide blast radius affecting multiple downstream systems.

## Overview

The Demo API is a simple blogging platform with users, posts, and comments. It demonstrates:

- **Authentication System**: Token-based JWT auth with password hashing
- **User Management**: User registration, profile management, admin privileges
- **Content Management**: Posts and comments with ownership and permissions
- **Middleware Layer**: Request validation, error handling, logging
- **Database Access Layer**: In-memory data persistence with CRUD operations

## Architecture

```
app.py (main)
├── config.py (configuration)
├── auth.py (authentication functions)
├── models.py (data structures)
├── database.py (CRUD operations)
├── middleware.py (request/response handling)
└── routes/
    ├── auth_routes.py (login/register)
    ├── user_routes.py (user management)
    └── post_routes.py (posts/comments)
```

## Key Dependencies for Impact Analysis

The architecture creates interesting call chains:

1. **Auth Flow**: `auth.py` functions → `database.py` user operations → route handlers
2. **Request Protection**: `middleware.py` decorators → `auth.py` token verification → `database.py` user lookup
3. **Content Operations**: Route handlers → `database.py` CRUD operations → model conversions
4. **Admin Checks**: Admin decorators → `auth.py` token verification → `database.py` user retrieval

## API Endpoints

### Authentication
- `POST /api/auth/register` - Register a new user
- `POST /api/auth/login` - Login and get JWT token
- `GET /api/auth/me` - Get current user profile
- `POST /api/auth/refresh` - Refresh JWT token

### Users
- `GET /api/users` - List all users (paginated)
- `GET /api/users/<id>` - Get user profile
- `PUT /api/users/<id>` - Update user
- `DELETE /api/users/<id>` - Delete user (admin only)
- `GET /api/users/<id>/posts` - Get user's posts
- `POST /api/users/<id>/make-admin` - Make user admin (admin only)
- `POST /api/users/<id>/deactivate` - Deactivate user (admin only)

### Posts
- `GET /api/posts` - List published posts (paginated)
- `POST /api/posts` - Create new post (authenticated)
- `GET /api/posts/<id>` - Get post with comments
- `PUT /api/posts/<id>` - Update post (author only)
- `DELETE /api/posts/<id>` - Delete post (author only)
- `GET /api/posts/<id>/comments` - Get post comments
- `POST /api/posts/<id>/comments` - Create comment (authenticated)
- `PUT /api/posts/comments/<id>` - Update comment (author only)
- `DELETE /api/posts/comments/<id>` - Delete comment (author only)

## Running with Myceliums

### Analyze the codebase
```bash
myc analyze ./examples/demo-api
```

### Run impact analysis with sample diff
```bash
myc impact --diff ./examples/demo-api/sample.diff
```

### Query the graph
```bash
# Find all functions that call verify_password
myc query "MATCH (n)-[:CALLS]->(m {name: 'verify_password'}) RETURN n.name"

# Find all downstream callers of hash_password
myc query "MATCH (m {name: 'hash_password'})-[:CALLS*]->(n) RETURN n.name"

# Find community structure
myc communities
```

## Impact Blast Radius Example

Changing the authentication functions (`hash_password`, `verify_password`, `generate_jwt_token`) creates a wide impact:

**Direct changes**: 3 functions in `auth.py`

**Immediate callers**: 
- `register()` in `auth_routes.py`
- `login()` in `auth_routes.py`
- `require_auth()` in `middleware.py`
- `require_admin()` in `middleware.py`

**Downstream impact**:
- All route handlers using `@require_auth` decorator
- All route handlers using `@require_admin` decorator
- Any code that needs to verify user identity

**Total affected symbols**: 15+ including routes, middleware, and API endpoints

## File Structure

| File | Purpose | Key Functions |
|------|---------|---|
| `app.py` | Application factory | `create_app()` |
| `config.py` | Configuration classes | `Config`, `DevelopmentConfig`, `TestingConfig`, `ProductionConfig` |
| `auth.py` | Authentication logic | `hash_password()`, `verify_password()`, `generate_jwt_token()`, `verify_jwt_token()`, `get_user_from_token()` |
| `models.py` | Data models | `User`, `Post`, `Comment` |
| `database.py` | Data access layer | CRUD operations for users, posts, comments |
| `middleware.py` | Request/response handling | `require_auth()`, `require_admin()`, `validate_json()`, error handlers |
| `routes/auth_routes.py` | Authentication endpoints | `register()`, `login()`, `get_current_user()`, `refresh_token()` |
| `routes/user_routes.py` | User management endpoints | `list_users()`, `get_user()`, `update_user()`, `delete_user()`, etc. |
| `routes/post_routes.py` | Post/comment endpoints | `list_posts()`, `create_post()`, `create_comment()`, etc. |

## Testing Impact Analysis

The `sample.diff` file demonstrates a change to core authentication:

1. Modifies `hash_password()` to add custom salting
2. Modifies `verify_password()` to use the new hash format
3. Changes `generate_jwt_token()` to extend expiration

This seemingly small change affects:
- User registration (uses `hash_password()`)
- User login (uses `verify_password()`)
- All authenticated endpoints (uses `generate_jwt_token()` validation)
- Admin operations (uses token verification)
- Token refresh (regenerates with new expiration)

## Notes

- All functions have clear docstrings for better graph representation
- Call chains are realistic and show typical API architecture patterns
- The in-memory database is intentionally simple to keep focus on code structure
- This demo is designed for impact analysis, not production use
