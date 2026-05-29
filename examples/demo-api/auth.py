"""
Authentication module for the Demo API.

Handles user authentication, password hashing, and JWT token operations.
"""

import hashlib
import hmac
import json
from datetime import datetime, timedelta
from typing import Dict, Optional


def hash_password(password: str, salt: str = '') -> str:
    """
    Hash a password using SHA256.
    
    Args:
        password: The password to hash
        salt: Optional salt for hashing
        
    Returns:
        Hashed password
    """
    if not salt:
        salt = 'demo-salt'
    
    return hashlib.sha256((salt + password).encode()).hexdigest()


def verify_password(password: str, hash_value: str) -> bool:
    """
    Verify a password against a hash.
    
    Args:
        password: The password to verify
        hash_value: The stored hash value
        
    Returns:
        True if password matches hash, False otherwise
    """
    return hash_password(password) == hash_value


def generate_jwt_token(user_id: int, secret: str = 'demo-jwt-secret') -> str:
    """
    Generate a JWT token for a user.
    
    Args:
        user_id: The user ID to encode in the token
        secret: The secret key for signing
        
    Returns:
        JWT token string
    """
    payload = {
        'user_id': user_id,
        'exp': datetime.utcnow() + timedelta(hours=24),
        'iat': datetime.utcnow()
    }
    
    # Simplified JWT encoding (not cryptographically secure for production)
    header = json.dumps({'alg': 'HS256', 'typ': 'JWT'})
    body = json.dumps(payload, default=str)
    
    message = f"{header}.{body}"
    signature = hmac.new(
        secret.encode(),
        message.encode(),
        hashlib.sha256
    ).hexdigest()
    
    return f"{message}.{signature}"


def verify_jwt_token(token: str, secret: str = 'demo-jwt-secret') -> Optional[Dict]:
    """
    Verify and decode a JWT token.
    
    Args:
        token: The JWT token to verify
        secret: The secret key for verification
        
    Returns:
        Decoded payload if valid, None otherwise
    """
    try:
        parts = token.split('.')
        if len(parts) != 3:
            return None
        
        header, body, signature = parts
        
        # Verify signature
        message = f"{header}.{body}"
        expected_signature = hmac.new(
            secret.encode(),
            message.encode(),
            hashlib.sha256
        ).hexdigest()
        
        if not hmac.compare_digest(signature, expected_signature):
            return None
        
        payload = json.loads(body)
        
        # Check expiration
        if 'exp' in payload:
            exp = datetime.fromisoformat(payload['exp'])
            if exp < datetime.utcnow():
                return None
        
        return payload
    
    except (ValueError, json.JSONDecodeError):
        return None


def get_user_from_token(token: str) -> Optional[int]:
    """
    Extract user ID from a valid JWT token.
    
    Args:
        token: The JWT token
        
    Returns:
        User ID if token is valid, None otherwise
    """
    payload = verify_jwt_token(token)
    if payload:
        return payload.get('user_id')
    return None
