"""
Main application entry point for the Demo API.

This is the core application module that initializes the Flask app,
registers blueprints, and sets up middleware.
"""

from flask import Flask
from .routes import auth_routes, user_routes, post_routes
from .middleware import setup_error_handlers, setup_logging
from .config import Config


def create_app(config=None):
    """
    Application factory function that creates and configures the Flask app.
    
    Args:
        config: Optional configuration object
        
    Returns:
        Configured Flask application
    """
    app = Flask(__name__)
    
    if config:
        app.config.from_object(config)
    else:
        app.config.from_object(Config)
    
    # Register middleware
    setup_logging(app)
    setup_error_handlers(app)
    
    # Register blueprints
    app.register_blueprint(auth_routes.bp)
    app.register_blueprint(user_routes.bp)
    app.register_blueprint(post_routes.bp)
    
    return app


if __name__ == '__main__':
    app = create_app()
    app.run(debug=True)
