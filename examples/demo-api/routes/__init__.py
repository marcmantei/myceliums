"""
Authentication routes for the Demo API.

Handles user registration and login endpoints.
"""

from flask import Blueprint, request, jsonify, g
from ..auth import hash_password, verify_password, generate_jwt_token
from ..middleware import validate_json, require_auth, log_request
from .. import database


bp = Blueprint('auth', __name__, url_prefix='/api/auth')


@bp.route('/register', methods=['POST'])
@validate_json
def register():
    """Register a new user."""
    log_request()
    
    data = request.get_json()
    
    # Validate required fields
    if not all(k in data for k in ['username', 'email', 'password']):
        return jsonify({'error': 'Missing required fields'}), 400
    
    username = data['username']
    email = data['email']
    password = data['password']
    
    # Validate input
    if len(username) < 3:
        return jsonify({'error': 'Username must be at least 3 characters'}), 400
    
    if len(password) < 6:
        return jsonify({'error': 'Password must be at least 6 characters'}), 400
    
    # Check if user already exists
    if database.get_user_by_username(username):
        return jsonify({'error': 'Username already exists'}), 409
    
    if database.get_user_by_email(email):
        return jsonify({'error': 'Email already registered'}), 409
    
    # Hash password and create user
    password_hash = hash_password(password)
    user = database.create_user(username, email, password_hash)
    
    return jsonify(user.to_dict()), 201


@bp.route('/login', methods=['POST'])
@validate_json
def login():
    """Login a user and return JWT token."""
    log_request()
    
    data = request.get_json()
    
    # Validate required fields
    if not all(k in data for k in ['username', 'password']):
        return jsonify({'error': 'Missing required fields'}), 400
    
    username = data['username']
    password = data['password']
    
    # Find user
    user = database.get_user_by_username(username)
    if not user:
        return jsonify({'error': 'Invalid credentials'}), 401
    
    # Verify password
    if not verify_password(password, user.password_hash):
        return jsonify({'error': 'Invalid credentials'}), 401
    
    # Check if user is active
    if not user.is_active:
        return jsonify({'error': 'User account is inactive'}), 403
    
    # Generate token
    token = generate_jwt_token(user.id)
    
    return jsonify({
        'token': token,
        'user': user.to_dict()
    }), 200


@bp.route('/me', methods=['GET'])
@require_auth
def get_current_user():
    """Get the current authenticated user."""
    log_request()
    
    user = database.get_user(g.user_id)
    if not user:
        return jsonify({'error': 'User not found'}), 404
    
    return jsonify(user.to_dict()), 200


@bp.route('/refresh', methods=['POST'])
@require_auth
def refresh_token():
    """Refresh the JWT token."""
    log_request()
    
    user = database.get_user(g.user_id)
    if not user:
        return jsonify({'error': 'User not found'}), 404
    
    token = generate_jwt_token(user.id)
    
    return jsonify({'token': token}), 200
