"""
Database models for the Demo API.

Defines the data structures for users, posts, and comments.
"""

from dataclasses import dataclass
from datetime import datetime
from typing import List, Optional


@dataclass
class User:
    """User model."""
    
    id: int
    username: str
    email: str
    password_hash: str
    created_at: datetime
    is_active: bool = True
    is_admin: bool = False
    
    def to_dict(self, include_password=False) -> dict:
        """Convert user to dictionary."""
        data = {
            'id': self.id,
            'username': self.username,
            'email': self.email,
            'created_at': self.created_at.isoformat(),
            'is_active': self.is_active,
            'is_admin': self.is_admin,
        }
        
        if include_password:
            data['password_hash'] = self.password_hash
        
        return data


@dataclass
class Post:
    """Post model."""
    
    id: int
    user_id: int
    title: str
    content: str
    created_at: datetime
    updated_at: Optional[datetime] = None
    is_published: bool = True
    view_count: int = 0
    
    def to_dict(self) -> dict:
        """Convert post to dictionary."""
        return {
            'id': self.id,
            'user_id': self.user_id,
            'title': self.title,
            'content': self.content,
            'created_at': self.created_at.isoformat(),
            'updated_at': self.updated_at.isoformat() if self.updated_at else None,
            'is_published': self.is_published,
            'view_count': self.view_count,
        }


@dataclass
class Comment:
    """Comment model."""
    
    id: int
    post_id: int
    user_id: int
    content: str
    created_at: datetime
    updated_at: Optional[datetime] = None
    
    def to_dict(self) -> dict:
        """Convert comment to dictionary."""
        return {
            'id': self.id,
            'post_id': self.post_id,
            'user_id': self.user_id,
            'content': self.content,
            'created_at': self.created_at.isoformat(),
            'updated_at': self.updated_at.isoformat() if self.updated_at else None,
        }
