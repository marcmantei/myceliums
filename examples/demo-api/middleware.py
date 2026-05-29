"""
Middleware and request/response handling for the Demo API.

Includes error handling, logging, and request validation middleware.
"""

from functools import wraps
from flask import request, jsonify, g
import logging
from datetime import datetime
from typing import Optional, Dict, Any
from .auth import get_user_from_token


def setup_logging(app):
    """Setup logging for the application."""
    logging.basicConfig(
        level=logging.INFO,
        format='%(asctime)s - %(name)s - %(levelname)s - %(message)s'
    )
    app.logger.info('Application logging initialized')


def setup_error_handlers(app):
    """Setup error handlers for the application."""
    
    @app.errorhandler(400)
    def bad_request(e):
        return jsonify({'error': 'Bad request'}), 400
    
    @app.errorhandler(401)
    def unauthorized(e):
        return jsonify({'error': 'Unauthorized'}), 401
    
    @app.errorhandler(403)
    def forbidden(e):
        return jsonify({'error': 'Forbidden'}), 403
    
    @app.errorhandler(404)
    def not_found(e):
        return jsonify({'error': 'Not found'}), 404
    
    @app.errorhandler(500)
    def internal_error(e):
        return jsonify({'error': 'Internal server error'}), 500


def require_auth(f):
    """Decorator to require authentication for a route."""
    @wraps(f)
    def decorated_function(*args, **kwargs):
        auth_header = request.headers.get('Authorization')
        
        if not auth_header:
            return jsonify({'error': 'Missing authorization header'}), 401
        
        try:
            scheme, token = auth_header.split()
            if scheme.lower() != 'bearer':
                return jsonify({'error': 'Invalid authorization scheme'}), 401
        except ValueError:
            return jsonify({'error': 'Invalid authorization header'}), 401
        
        user_id = get_user_from_token(token)
        if not user_id:
            return jsonify({'error': 'Invalid token'}), 401
        
        g.user_id = user_id
        return f(*args, **kwargs)
    
    return decorated_function


def require_admin(f):
    """Decorator to require admin privileges for a route."""
    @wraps(f)
    def decorated_function(*args, **kwargs):
        # First check authentication
        auth_header = request.headers.get('Authorization')
        
        if not auth_header:
            return jsonify({'error': 'Missing authorization header'}), 401
        
        try:
            scheme, token = auth_header.split()
            if scheme.lower() != 'bearer':
                return jsonify({'error': 'Invalid authorization scheme'}), 401
        except ValueError:
            return jsonify({'error': 'Invalid authorization header'}), 401
        
        user_id = get_user_from_token(token)
        if not user_id:
            return jsonify({'error': 'Invalid token'}), 401
        
        # Then check admin status
        from . import database
        user = database.get_user(user_id)
        if not user or not user.is_admin:
            return jsonify({'error': 'Admin privileges required'}), 403
        
        g.user_id = user_id
        return f(*args, **kwargs)
    
    return decorated_function


def log_request(level=logging.INFO):
    """Log incoming requests."""
    logger = logging.getLogger(__name__)
    logger.log(
        level,
        f'{request.method} {request.path} from {request.remote_addr}'
    )


def log_response(response):
    """Log outgoing responses."""
    logger = logging.getLogger(__name__)
    logger.info(
        f'Response: {response.status_code} for {request.method} {request.path}'
    )
    return response


def validate_json(f):
    """Decorator to validate that request contains JSON."""
    @wraps(f)
    def decorated_function(*args, **kwargs):
        if not request.is_json:
            return jsonify({'error': 'Request must be JSON'}), 400
        return f(*args, **kwargs)
    
    return decorated_function


def get_pagination_params() -> Dict[str, int]:
    """Extract and validate pagination parameters from request."""
    page = request.args.get('page', 1, type=int)
    per_page = request.args.get('per_page', 10, type=int)
    
    # Validate
    if page < 1:
        page = 1
    if per_page < 1 or per_page > 100:
        per_page = 10
    
    return {'page': page, 'per_page': per_page}


def paginate(items: list, page: int, per_page: int) -> Dict[str, Any]:
    """Paginate a list of items."""
    total = len(items)
    start = (page - 1) * per_page
    end = start + per_page
    
    return {
        'items': items[start:end],
        'page': page,
        'per_page': per_page,
        'total': total,
        'pages': (total + per_page - 1) // per_page
    }
