"""
User management routes for the Demo API.

Handles user profile, update, and deletion endpoints.
"""

from flask import Blueprint, request, jsonify, g
from ...middleware import require_auth, require_admin, validate_json, log_request, get_pagination_params, paginate
from ... import database


bp = Blueprint('users', __name__, url_prefix='/api/users')


@bp.route('', methods=['GET'])
def list_users():
    """List all users (paginated)."""
    log_request()
    
    params = get_pagination_params()
    all_users = list(database._users.values())
    
    paginated = paginate(
        [u.to_dict() for u in all_users],
        params['page'],
        params['per_page']
    )
    
    return jsonify(paginated), 200


@bp.route('/<int:user_id>', methods=['GET'])
def get_user(user_id):
    """Get a specific user."""
    log_request()
    
    user = database.get_user(user_id)
    if not user:
        return jsonify({'error': 'User not found'}), 404
    
    return jsonify(user.to_dict()), 200


@bp.route('/<int:user_id>', methods=['PUT'])
@require_auth
@validate_json
def update_user(user_id):
    """Update a user (only own profile or admin)."""
    log_request()
    
    # Check permissions
    if g.user_id != user_id:
        user = database.get_user(g.user_id)
        if not user or not user.is_admin:
            return jsonify({'error': 'Cannot modify other users'}), 403
    
    user = database.get_user(user_id)
    if not user:
        return jsonify({'error': 'User not found'}), 404
    
    data = request.get_json()
    
    # Allow updating certain fields
    allowed_fields = ['email', 'is_active']
    updates = {k: v for k, v in data.items() if k in allowed_fields}
    
    updated_user = database.update_user(user_id, **updates)
    
    return jsonify(updated_user.to_dict()), 200


@bp.route('/<int:user_id>', methods=['DELETE'])
@require_admin
def delete_user(user_id):
    """Delete a user (admin only)."""
    log_request()
    
    user = database.get_user(user_id)
    if not user:
        return jsonify({'error': 'User not found'}), 404
    
    database.delete_user(user_id)
    
    return jsonify({'message': 'User deleted'}), 200


@bp.route('/<int:user_id>/posts', methods=['GET'])
def get_user_posts(user_id):
    """Get all posts by a user."""
    log_request()
    
    user = database.get_user(user_id)
    if not user:
        return jsonify({'error': 'User not found'}), 404
    
    posts = database.get_posts_by_user(user_id)
    
    return jsonify({
        'user_id': user_id,
        'posts': [p.to_dict() for p in posts]
    }), 200


@bp.route('/<int:user_id>/make-admin', methods=['POST'])
@require_admin
def make_admin(user_id):
    """Make a user an admin (admin only)."""
    log_request()
    
    user = database.get_user(user_id)
    if not user:
        return jsonify({'error': 'User not found'}), 404
    
    database.update_user(user_id, is_admin=True)
    
    return jsonify({'message': 'User is now admin'}), 200


@bp.route('/<int:user_id>/deactivate', methods=['POST'])
@require_admin
def deactivate_user(user_id):
    """Deactivate a user account (admin only)."""
    log_request()
    
    user = database.get_user(user_id)
    if not user:
        return jsonify({'error': 'User not found'}), 404
    
    database.update_user(user_id, is_active=False)
    
    return jsonify({'message': 'User deactivated'}), 200
