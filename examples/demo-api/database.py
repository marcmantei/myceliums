"""
Database access layer for the Demo API.

Provides methods for accessing and manipulating user and post data.
"""

from datetime import datetime
from typing import List, Optional, Dict, Any
from .models import User, Post, Comment


# In-memory storage for demo purposes
_users: Dict[int, User] = {}
_posts: Dict[int, Post] = {}
_comments: Dict[int, Comment] = {}
_user_id_counter = 1
_post_id_counter = 1
_comment_id_counter = 1


def create_user(username: str, email: str, password_hash: str) -> User:
    """Create a new user."""
    global _user_id_counter
    
    user = User(
        id=_user_id_counter,
        username=username,
        email=email,
        password_hash=password_hash,
        created_at=datetime.utcnow()
    )
    
    _users[_user_id_counter] = user
    _user_id_counter += 1
    
    return user


def get_user(user_id: int) -> Optional[User]:
    """Get a user by ID."""
    return _users.get(user_id)


def get_user_by_username(username: str) -> Optional[User]:
    """Get a user by username."""
    for user in _users.values():
        if user.username == username:
            return user
    return None


def get_user_by_email(email: str) -> Optional[User]:
    """Get a user by email."""
    for user in _users.values():
        if user.email == email:
            return user
    return None


def update_user(user_id: int, **kwargs) -> Optional[User]:
    """Update a user's fields."""
    user = get_user(user_id)
    if not user:
        return None
    
    for key, value in kwargs.items():
        if hasattr(user, key):
            setattr(user, key, value)
    
    return user


def delete_user(user_id: int) -> bool:
    """Delete a user."""
    if user_id in _users:
        del _users[user_id]
        return True
    return False


def create_post(user_id: int, title: str, content: str) -> Post:
    """Create a new post."""
    global _post_id_counter
    
    post = Post(
        id=_post_id_counter,
        user_id=user_id,
        title=title,
        content=content,
        created_at=datetime.utcnow()
    )
    
    _posts[_post_id_counter] = post
    _post_id_counter += 1
    
    return post


def get_post(post_id: int) -> Optional[Post]:
    """Get a post by ID."""
    return _posts.get(post_id)


def get_posts_by_user(user_id: int) -> List[Post]:
    """Get all posts by a user."""
    return [p for p in _posts.values() if p.user_id == user_id]


def get_all_posts(published_only=True) -> List[Post]:
    """Get all posts."""
    posts = list(_posts.values())
    if published_only:
        posts = [p for p in posts if p.is_published]
    return sorted(posts, key=lambda p: p.created_at, reverse=True)


def update_post(post_id: int, **kwargs) -> Optional[Post]:
    """Update a post's fields."""
    post = get_post(post_id)
    if not post:
        return None
    
    kwargs['updated_at'] = datetime.utcnow()
    
    for key, value in kwargs.items():
        if hasattr(post, key):
            setattr(post, key, value)
    
    return post


def increment_view_count(post_id: int) -> Optional[Post]:
    """Increment the view count of a post."""
    post = get_post(post_id)
    if post:
        post.view_count += 1
    return post


def delete_post(post_id: int) -> bool:
    """Delete a post."""
    if post_id in _posts:
        del _posts[post_id]
        # Also delete associated comments
        comment_ids = [c for c in _comments.keys() if _comments[c].post_id == post_id]
        for cid in comment_ids:
            del _comments[cid]
        return True
    return False


def create_comment(post_id: int, user_id: int, content: str) -> Comment:
    """Create a new comment."""
    global _comment_id_counter
    
    comment = Comment(
        id=_comment_id_counter,
        post_id=post_id,
        user_id=user_id,
        content=content,
        created_at=datetime.utcnow()
    )
    
    _comments[_comment_id_counter] = comment
    _comment_id_counter += 1
    
    return comment


def get_comment(comment_id: int) -> Optional[Comment]:
    """Get a comment by ID."""
    return _comments.get(comment_id)


def get_comments_for_post(post_id: int) -> List[Comment]:
    """Get all comments for a post."""
    return [c for c in _comments.values() if c.post_id == post_id]


def update_comment(comment_id: int, **kwargs) -> Optional[Comment]:
    """Update a comment's fields."""
    comment = get_comment(comment_id)
    if not comment:
        return None
    
    kwargs['updated_at'] = datetime.utcnow()
    
    for key, value in kwargs.items():
        if hasattr(comment, key):
            setattr(comment, key, value)
    
    return comment


def delete_comment(comment_id: int) -> bool:
    """Delete a comment."""
    if comment_id in _comments:
        del _comments[comment_id]
        return True
    return False
