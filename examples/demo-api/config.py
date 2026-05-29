"""
Configuration module for the Demo API.

Stores all configuration settings for the application.
"""


class Config:
    """Base configuration."""
    
    DEBUG = False
    TESTING = False
    SECRET_KEY = 'demo-secret-key-for-testing'
    DATABASE_URL = 'sqlite:///demo.db'
    JWT_SECRET = 'demo-jwt-secret'
    JWT_ALGORITHM = 'HS256'
    JWT_EXPIRATION_HOURS = 24


class DevelopmentConfig(Config):
    """Development configuration."""
    
    DEBUG = True
    TESTING = False


class TestingConfig(Config):
    """Testing configuration."""
    
    DEBUG = True
    TESTING = True
    DATABASE_URL = 'sqlite:///:memory:'


class ProductionConfig(Config):
    """Production configuration."""
    
    DEBUG = False
    TESTING = False
