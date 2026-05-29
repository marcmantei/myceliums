"""
Post and comment routes for the Demo API.

Handles post creation, reading, updating, and comment management.
"""

from flask import Blueprint, request, jsonify, g
from ...middleware import require_auth, validate_json, log_request, get_pagination_params, paginate
from ... import database


bp = Blueprint('posts', __name__, url_prefix='/api/posts')


@bp.route('', methods=['GET'])
def list_posts():
    """List all published posts (paginated)."""
    log_request()
    
    params = get_pagination_params()
    all_posts = database.get_all_posts(published_only=True)
    
    paginated = paginate(
        [p.to_dict() for p in all_posts],
        params['page'],
        params['per_page']
    )
    
    return jsonify(paginated), 200


@bp.route('/<int:post_id>', methods=['GET'])
def get_post(post_id):
    """Get a specific post and increment view count."""
    log_request()
    
    post = database.get_post(post_id)
    if not post:
        return jsonify({'error': 'Post not found'}), 404
    
    if not post.is_published:
        return jsonify({'error': 'Post not found'}), 404
    
    # Increment view count
    database.increment_view_count(post_id)
    
    # Get comments
    comments = database.get_comments_for_post(post_id)
    
    return jsonify({
        'post': post.to_dict(),
        'comments': [c.to_dict() for c in comments]
    }), 200


@bp.route('', methods=['POST'])
@require_auth
@validate_json
def create_post():
    """Create a new post."""
    log_request()
    
    data = request.get_json()
    
    # Validate required fields
    if not all(k in data for k in ['title', 'content']):
        return jsonify({'error': 'Missing required fields'}), 400
    
    title = data['title']
    content = data['content']
    is_published = data.get('is_published', True)
    
    # Validate input
    if len(title) < 1 or len(title) > 200:
        return jsonify({'error': 'Title must be between 1 and 200 characters'}), 400
    
    if len(content) < 1:
        return jsonify({'error': 'Content cannot be empty'}), 400
    
    # Create post
    post = database.create_post(g.user_id, title, content)
    
    if not is_published:
        database.update_post(post.id, is_published=False)
    
    return jsonify(post.to_dict()), 201


@bp.route('/<int:post_id>', methods=['PUT'])
@require_auth
@validate_json
def update_post(post_id):
    """Update a post (only author)."""
    log_request()
    
    post = database.get_post(post_id)
    if not post:
        return jsonify({'error': 'Post not found'}), 404
    
    # Check permissions
    if post.user_id != g.user_id:
        return jsonify({'error': 'Cannot modify other users posts'}), 403
    
    data = request.get_json()
    
    # Allow updating certain fields
    allowed_fields = ['title', 'content', 'is_published']
    updates = {k: v for k, v in data.items() if k in allowed_fields}
    
    updated_post = database.update_post(post_id, **updates)
    
    return jsonify(updated_post.to_dict()), 200


@bp.route('/<int:post_id>', methods=['DELETE'])
@require_auth
def delete_post(post_id):
    """Delete a post (only author)."""
    log_request()
    
    post = database.get_post(post_id)
    if not post:
        return jsonify({'error': 'Post not found'}), 404
    
    # Check permissions
    if post.user_id != g.user_id:
        return jsonify({'error': 'Cannot delete other users posts'}), 403
    
    database.delete_post(post_id)
    
    return jsonify({'message': 'Post deleted'}), 200


@bp.route('/<int:post_id>/comments', methods=['GET'])
def get_post_comments(post_id):
    """Get all comments for a post."""
    log_request()
    
    post = database.get_post(post_id)
    if not post:
        return jsonify({'error': 'Post not found'}), 404
    
    comments = database.get_comments_for_post(post_id)
    
    return jsonify({
        'post_id': post_id,
        'comments': [c.to_dict() for c in comments]
    }), 200


@bp.route('/<int:post_id>/comments', methods=['POST'])
@require_auth
@validate_json
def create_comment(post_id):
    """Create a comment on a post."""
    log_request()
    
    post = database.get_post(post_id)
    if not post:
        return jsonify({'error': 'Post not found'}), 404
    
    data = request.get_json()
    
    # Validate required fields
    if 'content' not in data:
        return jsonify({'error': 'Content is required'}), 400
    
    content = data['content']
    
    # Validate input
    if len(content) < 1:
        return jsonify({'error': 'Comment cannot be empty'}), 400
    
    # Create comment
    comment = database.create_comment(post_id, g.user_id, content)
    
    return jsonify(comment.to_dict()), 201


@bp.route('/comments/<int:comment_id>', methods=['GET'])
def get_comment(comment_id):
    """Get a specific comment."""
    log_request()
    
    comment = database.get_comment(comment_id)
    if not comment:
        return jsonify({'error': 'Comment not found'}), 404
    
    return jsonify(comment.to_dict()), 200


@bp.route('/comments/<int:comment_id>', methods=['PUT'])
@require_auth
@validate_json
def update_comment(comment_id):
    """Update a comment (only author)."""
    log_request()
    
    comment = database.get_comment(comment_id)
    if not comment:
        return jsonify({'error': 'Comment not found'}), 404
    
    # Check permissions
    if comment.user_id != g.user_id:
        return jsonify({'error': 'Cannot modify other users comments'}), 403
    
    data = request.get_json()
    
    if 'content' not in data:
        return jsonify({'error': 'Content is required'}), 400
    
    content = data['content']
    
    if len(content) < 1:
        return jsonify({'error': 'Comment cannot be empty'}), 400
    
    updated_comment = database.update_comment(comment_id, content=content)
    
    return jsonify(updated_comment.to_dict()), 200


@bp.route('/comments/<int:comment_id>', methods=['DELETE'])
@require_auth
def delete_comment(comment_id):
    """Delete a comment (only author)."""
    log_request()
    
    comment = database.get_comment(comment_id)
    if not comment:
        return jsonify({'error': 'Comment not found'}), 404
    
    # Check permissions
    if comment.user_id != g.user_id:
        return jsonify({'error': 'Cannot delete other users comments'}), 403
    
    database.delete_comment(comment_id)
    
    return jsonify({'message': 'Comment deleted'}), 200
